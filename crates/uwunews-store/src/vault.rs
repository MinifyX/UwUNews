//! The vault key on this device.
//!
//! UwUNews keeps no master password of its own. The key arrives when this
//! device creates or joins a UwUSync account, or signs in to UwULock, and goes
//! straight into the [`SecretStore`](crate::SecretStore); every later start
//! takes it from there, so syncing needs no password. Which label set it goes
//! with — UwUSSH's or UwURDP's — is kept next to it in `meta.vault_flavor`.
//!
//! A UwUSync vault's header (salt, costs, wrapped key) is kept as well: it is
//! not secret, and it lets the person unlock again with the master password
//! should the secret store lose the key.

use crate::secret::{names, SecretStore};
use crate::{parse_uuid, Result, Store, StoreError};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use uuid::Uuid;
use uwunews_vault::{AccountKey, Flavor, KdfParams, Sealed, UnlockedVault, VaultHeader};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VaultStatus {
    /// No account yet: nothing to seal with.
    Absent,
    /// An account, but its key is not here: unlock with the master password
    /// (UwUSync) or sign in again (UwULock).
    Locked,
    Unlocked,
}

/// The vault key the secret store holds, if it holds one for this vault.
pub(crate) fn from_secrets(
    secrets: &dyn SecretStore,
    vault_id: &str,
    flavor: Option<&str>,
) -> Option<UnlockedVault> {
    let flavor = Flavor::parse(flavor?)?;
    let vault_id = Uuid::parse_str(vault_id).ok()?;
    let bytes = Zeroizing::new(secrets.get(names::VAULT_KEY)?);
    let key: [u8; 32] = bytes.as_slice().try_into().ok()?;
    Some(UnlockedVault::from_key(
        vault_id,
        Zeroizing::new(key),
        flavor,
    ))
}

impl Store {
    pub fn vault_status(&self) -> Result<VaultStatus> {
        if self.vault.lock().is_some() {
            return Ok(VaultStatus::Unlocked);
        }
        let flavor: Option<String> = self.conn.lock().query_row(
            "SELECT vault_flavor FROM meta WHERE id = 1",
            [],
            |row| row.get(0),
        )?;
        let paired = self.sync_state()?.paired() || self.lock_state()?.active;
        Ok(if flavor.is_some() || paired {
            VaultStatus::Locked
        } else {
            VaultStatus::Absent
        })
    }

    /// The id every synced row is written under.
    pub fn current_vault_id(&self) -> Result<Uuid> {
        parse_uuid(&crate::vault_id(&self.conn.lock())?)
    }

    /// Whose labels this vault uses, while a key for it is held.
    pub fn vault_flavor(&self) -> Option<Flavor> {
        self.vault.lock().as_ref().map(UnlockedVault::flavor)
    }

    /// The header of the UwUSync vault, as the server has it.
    pub fn vault_header(&self) -> Result<Option<VaultHeader>> {
        header_row(&self.conn.lock())
    }

    /// Open the vault with the master password — for a UwUSync device whose
    /// secret store lost the key — and keep the key again.
    pub fn unlock_vault(&self, password: &[u8], account_key: Option<&AccountKey>) -> Result<()> {
        let header = self.vault_header()?.ok_or(StoreError::NoVault)?;
        let unlocked = header.unlock_with(password, account_key)?;
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE meta SET vault_flavor = ?1 WHERE id = 1",
            [unlocked.flavor().as_str()],
        )?;
        self.secrets
            .set(names::VAULT_KEY, unlocked.export_key().as_slice());
        *self.vault.lock() = Some(unlocked);
        tracing::info!("vault unlocked");
        Ok(())
    }

    /// Keep a vault key: in memory, in the secret store, and its flavor in
    /// `meta`. The caller's transaction must have made `vault_id` current.
    pub(crate) fn hold_vault(&self, vault: UnlockedVault) {
        self.secrets
            .set(names::VAULT_KEY, vault.export_key().as_slice());
        *self.vault.lock() = Some(vault);
    }

    /// Drop the vault key everywhere — on disconnect.
    pub(crate) fn drop_vault(&self) {
        self.secrets.delete(names::VAULT_KEY);
        self.vault.lock().take();
    }
}

pub(crate) fn store_header(conn: &rusqlite::Connection, header: &VaultHeader) -> Result<()> {
    conn.execute("DELETE FROM vault", [])?;
    conn.execute(
        "INSERT INTO vault
            (id, vault_id, kdf_memory_kib, kdf_time_cost, kdf_parallelism,
             salt, wrapped_nonce, wrapped_blob, needs_account_key, flavor)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            header.vault_id.to_string(),
            header.kdf.memory_kib,
            header.kdf.time_cost,
            header.kdf.parallelism,
            header.salt.to_vec(),
            header.wrapped_key.nonce,
            header.wrapped_key.blob,
            header.needs_account_key,
            header.flavor.as_str(),
        ],
    )?;
    Ok(())
}

type HeaderRow = (String, KdfParams, Vec<u8>, Sealed, bool, String);

fn header_row(conn: &rusqlite::Connection) -> Result<Option<VaultHeader>> {
    let row: Option<HeaderRow> = conn
        .query_row(
            "SELECT vault_id, kdf_memory_kib, kdf_time_cost, kdf_parallelism,
                    salt, wrapped_nonce, wrapped_blob, needs_account_key, flavor
               FROM vault WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    KdfParams {
                        memory_kib: row.get(1)?,
                        time_cost: row.get(2)?,
                        parallelism: row.get(3)?,
                    },
                    row.get(4)?,
                    Sealed {
                        nonce: row.get(5)?,
                        blob: row.get(6)?,
                    },
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(vault_id, kdf, salt, wrapped_key, needs_account_key, flavor)| {
            // A header with absurd costs would make every unlock run out of memory.
            if !kdf.within_limits() {
                return Err(StoreError::Vault(uwunews_vault::VaultError::Kdf(
                    "the vault header asks for unreasonable key derivation costs".into(),
                )));
            }
            Ok(VaultHeader {
                vault_id: parse_uuid(&vault_id)?,
                kdf,
                salt: salt
                    .as_slice()
                    .try_into()
                    .map_err(|_| StoreError::NoVault)?,
                wrapped_key,
                needs_account_key,
                flavor: Flavor::parse(&flavor).ok_or(StoreError::NoVault)?,
            })
        },
    )
    .transpose()
}
