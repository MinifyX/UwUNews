//! The local store: one SQLite file with the feeds this account follows, the
//! collections over them, read and starred marks, and this device's cache of
//! fetched articles.
//!
//! Three kinds of rows travel — sources, collections and marks — and carry the
//! sync header from `uwunews-proto`: id, vault, hybrid logical clock,
//! revision, tombstone and an outbox flag set in the same transaction as the
//! write. They are plaintext here; they are sealed with the vault key only on
//! the way out (see [`sync`]). Articles, fetch validators and notification
//! bookkeeping are this device's alone.
//!
//! **No secret lives in SQLite.** The vault key, the UwUSync device and
//! account keys and the UwULock tokens go through the [`SecretStore`] the app
//! hands in.

mod articles;
mod collections;
pub mod lock;
pub mod manifest;
mod marks;
mod schema;
pub mod secret;
mod sources;
pub mod sync;
pub mod vault;

pub use articles::{ArticleDraft, ArticleQuery, ArticleRecord, FetchState, PruneReport};
pub use collections::{CollectionDraft, CollectionRecord, CollectionRules};
pub use lock::{LockAccount, LockEnrolment, LockState, Space};
pub use manifest::{manifest_id, ManifestFloor, Problem, Violation, Withheld};
pub use marks::{article_key, mark_id};
pub use schema::SCHEMA_VERSION;
pub use secret::{MemorySecrets, SecretStore};
pub use sources::{SourceDraft, SourceRecord};
pub use sync::{ApplyReport, Enrolment, EnrolmentKeys, Pushed, SyncState};
pub use uwunews_vault::Flavor;
pub use vault::VaultStatus;

use parking_lot::Mutex;
use rusqlite::{params, Connection, Transaction};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;
use uwunews_proto::Hlc;
use uwunews_vault::UnlockedVault;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("could not create the data directory: {0}")]
    Io(#[from] std::io::Error),
    /// A field failed validation.
    #[error("invalid {field}: {problem}")]
    Invalid {
        field: &'static str,
        problem: &'static str,
    },
    #[error("no source with id {0}")]
    UnknownSource(Uuid),
    #[error("no collection with id {0}")]
    UnknownCollection(Uuid),
    #[error("no article with key {0}")]
    UnknownArticle(String),
    #[error("that feed is followed already")]
    DuplicateSource,
    #[error(
        "this database was written by a newer UwUNews (schema {found}, this build knows {known})"
    )]
    SchemaTooNew { found: i64, known: i64 },
    #[error("the vault is locked")]
    VaultLocked,
    #[error("no vault has been set up yet")]
    NoVault,
    #[error(transparent)]
    Vault(#[from] uwunews_vault::VaultError),
}

pub type Result<T> = std::result::Result<T, StoreError>;

pub struct Store {
    // rusqlite's Connection is Send but not Sync. Every call is short, so one
    // lock is simpler than a pool.
    conn: Mutex<Connection>,
    device: u32,
    // The vault key in memory, when this device holds one. Locks are always
    // taken connection first, vault second.
    vault: Mutex<Option<UnlockedVault>>,
    secrets: Arc<dyn SecretStore>,
}

/// The database lists what someone reads; the default umask would leave it
/// readable by every other user on the machine. SQLite gives its `-wal` and
/// `-shm` files the mode of the database.
fn private_to_this_user(path: &Path, made_folder: bool) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        if let Some(parent) = path.parent().filter(|_| made_folder) {
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = (path, made_folder);
    Ok(())
}

impl Store {
    pub fn open(path: &Path, secrets: Arc<dyn SecretStore>) -> Result<Self> {
        let mut made_folder = false;
        if let Some(parent) = path.parent() {
            made_folder = !parent.as_os_str().is_empty() && !parent.exists();
            std::fs::create_dir_all(parent)?;
        }
        private_to_this_user(path, made_folder)?;
        let conn = Connection::open(path)?;
        // Readers never block the writer, and a crash mid-write cannot
        // corrupt the file.
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn, secrets)
    }

    pub fn open_in_memory(secrets: Arc<dyn SecretStore>) -> Result<Self> {
        Self::init(Connection::open_in_memory()?, secrets)
    }

    fn init(mut conn: Connection, secrets: Arc<dyn SecretStore>) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        schema::migrate(&mut conn)?;
        let (device, vault_id, flavor): (i64, String, Option<String>) = conn.query_row(
            "SELECT device_id, vault_id, vault_flavor FROM meta WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let vault = vault::from_secrets(&*secrets, &vault_id, flavor.as_deref());
        Ok(Self {
            conn: Mutex::new(conn),
            device: device as u32,
            vault: Mutex::new(vault),
            secrets,
        })
    }

    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .lock()
            .pragma_query_value(None, "user_version", |row| row.get(0))?)
    }

    /// The device part of every clock this install writes.
    pub fn device(&self) -> u32 {
        self.device
    }
}

/// Advance the local hybrid logical clock inside a write transaction.
pub(crate) fn tick(tx: &Transaction, device: u32) -> Result<Hlc> {
    let (wall, counter): (i64, i64) = tx.query_row(
        "SELECT clock_wall_ms, clock_counter FROM meta WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let next = Hlc::new(wall as u64, counter as u32, device).tick(now_ms());
    tx.execute(
        "UPDATE meta SET clock_wall_ms = ?1, clock_counter = ?2 WHERE id = 1",
        params![next.wall_ms as i64, next.counter as i64],
    )?;
    Ok(next)
}

pub(crate) fn vault_id(conn: &Connection) -> Result<String> {
    Ok(
        conn.query_row("SELECT vault_id FROM meta WHERE id = 1", [], |row| {
            row.get(0)
        })?,
    )
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

pub(crate) fn parse_uuid(text: &str) -> Result<Uuid> {
    Uuid::parse_str(text).map_err(|_| StoreError::Invalid {
        field: "id",
        problem: "invalid",
    })
}
