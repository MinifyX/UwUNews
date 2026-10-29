//! What the sync engine needs from the store: records on their way out, and
//! records on their way in.
//!
//! A local write marks its row `dirty` in the same transaction that writes it,
//! so the outbox is the database rather than something next to it that a crash
//! can lose. [`Store::pending_envelopes`] turns those rows into sealed
//! envelopes, and [`Store::apply_envelopes`] takes envelopes apart again — one
//! decision per record, inside one transaction, so a sync interrupted halfway
//! leaves no half-applied record behind.
//!
//! - **The server never sees a field.** Every payload is sealed with the vault
//!   key before it leaves, so pushing needs the key, and so does applying.
//! - **An envelope that fails its seal is dropped, not applied.** The header —
//!   clock and tombstone flag included — is part of the associated data, so a
//!   server cannot mark a source deleted or replay an old version.
//! - **Kinds this build does not keep are skipped.** A vault shared with
//!   UwUSSH or UwURDP holds their records too; the server hands them only to
//!   clients that ask, and any that arrive anyway are passed over.
//! - **Fields a newer build wrote survive.** What a payload carries that this
//!   build does not know is kept in `sync_extra` and written back out.

use crate::marks::mark_id;
use crate::secret::names;
use crate::sources::drop_source_cache;
use crate::{now_ms, parse_uuid, Result, Store, StoreError};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::Serialize;
use uuid::Uuid;
use uwunews_proto::{
    resolve, EntityKind, Envelope, Extra, Hlc, NewsCollectionPayload, NewsMarkPayload,
    NewsRulesPayload, NewsSourcePayload, Resolution, Version, MAX_BATCH_BYTES, MAX_BLOB_BYTES,
};
use uwunews_vault::{AccountKey, Flavor, Sealed, UnlockedVault, VaultHeader};
use zeroize::Zeroizing;

/// Where this device stands with its UwUSync server. Local, never synced.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    pub server_url: Option<String>,
    pub account_id: Option<Uuid>,
    pub device_id: Option<Uuid>,
    /// The fingerprint of the certificate this device pinned when it paired.
    /// Anything else answering at that address is not this server.
    pub tls_fingerprint: Option<String>,
    /// The server sequence number everything up to which this device has seen.
    pub cursor: u64,
    pub last_sync_ms: Option<u64>,
    pub paired_ms: Option<u64>,
}

impl SyncState {
    /// Whether this device is set up to sync with a UwUSync server.
    pub fn paired(&self) -> bool {
        self.server_url.is_some() && self.account_id.is_some() && self.device_id.is_some()
    }
}

/// What a pairing agreed on, on its way into the database.
#[derive(Debug, Clone)]
pub struct Enrolment {
    pub server_url: String,
    pub account_id: Uuid,
    pub device_id: Uuid,
    /// `None` when the server's certificate comes from a real authority and
    /// there is nothing to pin.
    pub tls_fingerprint: Option<String>,
}

/// The two secrets a paired device keeps, in the [`SecretStore`](crate::SecretStore).
pub struct EnrolmentKeys {
    /// The Ed25519 seed this device signs the server's challenges with.
    pub device_key: Zeroizing<[u8; 32]>,
    pub account_key: Option<AccountKey>,
}

impl std::fmt::Debug for EnrolmentKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EnrolmentKeys(redacted)")
    }
}

/// What one record the server accepted is called, so the store can clear the
/// right row — and only if it has not changed since.
#[derive(Debug, Clone, Copy)]
pub struct Pushed {
    pub id: Uuid,
    pub kind: EntityKind,
    pub updated_at: Hlc,
    pub seq: u64,
}

/// What came of a batch of pulled records.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    /// Records written: new ones, updates and tombstones.
    pub applied: usize,
    /// Records where this device's version was the newer one and stays.
    pub kept: usize,
    /// Records that were already what we have.
    pub identical: usize,
    /// Records of a kind this build does not keep, a tombstone for something
    /// we never had, or a mark pruned here.
    pub skipped: usize,
    /// Records whose seal did not hold, or that belong to another vault.
    /// Anything above zero means the server handed back something it should
    /// not have.
    pub rejected: usize,
    /// Other devices' manifests that came along, whatever became of them.
    pub manifests: usize,
}

impl ApplyReport {
    /// Fold another batch's counts into these.
    pub fn add(&mut self, other: ApplyReport) {
        self.applied += other.applied;
        self.kept += other.kept;
        self.identical += other.identical;
        self.skipped += other.skipped;
        self.rejected += other.rejected;
        self.manifests += other.manifests;
    }
}

/// The table a kind lives in.
pub(crate) fn table_of(kind: EntityKind) -> Option<&'static str> {
    Some(match kind {
        EntityKind::NewsSource => "news_sources",
        EntityKind::NewsCollection => "news_collections",
        EntityKind::NewsMark => "news_marks",
        // Travels like a record, but is no row of the vault: not pending in
        // the count, and pushed on its own (see `crate::manifest`).
        EntityKind::Manifest => "manifests",
    })
}

/// Every table whose rows travel, in the order records are applied.
pub(crate) const SYNCED_KINDS: [EntityKind; 3] = EntityKind::APPLY_ORDER;

/// A row waiting to be pushed.
struct Pending {
    id: Uuid,
    kind: EntityKind,
    updated_at: Hlc,
    deleted: bool,
    base_seq: u64,
    payload: Zeroizing<Vec<u8>>,
}

pub(crate) fn clock_of(wall: i64, counter: i64, device: i64) -> Hlc {
    Hlc::new(wall as u64, counter as u32, device as u32)
}

/// `sync_extra` as it comes out of the database: the fields a newer build
/// wrote that this one does not know.
fn extra_from(text: Option<String>) -> Extra {
    text.and_then(|text| serde_json::from_str::<Extra>(&text).ok())
        .unwrap_or_default()
}

fn extra_to(extra: &Extra) -> Option<String> {
    if extra.is_empty() {
        None
    } else {
        serde_json::to_string(extra).ok()
    }
}

impl Store {
    pub fn sync_state(&self) -> Result<SyncState> {
        let conn = self.conn.lock();
        Ok(conn.query_row(
            "SELECT server_url, account_id, device_id, cursor, last_sync_ms,
                    tls_fingerprint, paired_ms
               FROM sync_state WHERE id = 1",
            [],
            |row| {
                let account: Option<String> = row.get(1)?;
                let device: Option<String> = row.get(2)?;
                let cursor: i64 = row.get(3)?;
                let last: Option<i64> = row.get(4)?;
                let paired: Option<i64> = row.get(6)?;
                Ok(SyncState {
                    server_url: row.get(0)?,
                    account_id: account.and_then(|id| Uuid::parse_str(&id).ok()),
                    device_id: device.and_then(|id| Uuid::parse_str(&id).ok()),
                    tls_fingerprint: row.get(5)?,
                    cursor: cursor as u64,
                    last_sync_ms: last.map(|ms| ms as u64),
                    paired_ms: paired.map(|ms| ms as u64),
                })
            },
        )?)
    }

    /// Remember a pairing: where the server is and who this device is there,
    /// with the two secrets in the secret store. UwUSync becomes the backend;
    /// a UwULock sign-in this device had stops being used.
    pub fn save_enrolment(
        &self,
        enrolment: &Enrolment,
        device_key: &[u8; 32],
        account_key: Option<&AccountKey>,
    ) -> Result<()> {
        self.secrets.set(names::DEVICE_KEY, device_key);
        match account_key {
            Some(key) => self.secrets.set(names::ACCOUNT_KEY, key.as_bytes()),
            None => self.secrets.delete(names::ACCOUNT_KEY),
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE sync_state
                SET server_url = ?1, account_id = ?2, device_id = ?3, tls_fingerprint = ?4,
                    paired_ms = ?5
              WHERE id = 1",
            params![
                enrolment.server_url,
                enrolment.account_id.to_string(),
                enrolment.device_id.to_string(),
                enrolment.tls_fingerprint,
                now_ms() as i64,
            ],
        )?;
        tx.execute(
            "UPDATE lock_state SET active = 0, signed_in_ms = NULL WHERE id = 1",
            [],
        )?;
        tx.commit()?;
        self.secrets.delete(names::LOCK_REFRESH_TOKEN);
        tracing::info!(
            account = %enrolment.account_id,
            device = %enrolment.device_id,
            "this device is paired with a server"
        );
        Ok(())
    }

    /// The keys a paired device signs and unlocks with. `None` when this
    /// device is not paired, or when the secret store no longer has them —
    /// then the device has to be added to the account again.
    pub fn enrolment_keys(&self) -> Result<Option<EnrolmentKeys>> {
        if !self.sync_state()?.paired() {
            return Ok(None);
        }
        let Some(device) = self.secrets.get(names::DEVICE_KEY).map(Zeroizing::new) else {
            tracing::warn!("the secret store lost this device's key");
            return Ok(None);
        };
        let Ok(device_key) = <[u8; 32]>::try_from(device.as_slice()) else {
            return Ok(None);
        };
        let account_key = self
            .secrets
            .get(names::ACCOUNT_KEY)
            .map(Zeroizing::new)
            .and_then(|bytes| <[u8; 16]>::try_from(bytes.as_slice()).ok())
            .map(AccountKey::from_bytes);
        Ok(Some(EnrolmentKeys {
            device_key: Zeroizing::new(device_key),
            account_key,
        }))
    }

    /// Unpair from UwUSync: forget the server, the device identity and both
    /// secrets. The records and the vault key stay.
    pub fn forget_enrolment(&self) -> Result<()> {
        let conn = self.conn.lock();
        forget_uwusync(&conn)?;
        drop(conn);
        self.secrets.delete(names::DEVICE_KEY);
        self.secrets.delete(names::ACCOUNT_KEY);
        tracing::info!("this device is no longer paired");
        Ok(())
    }

    /// Stop syncing altogether, whichever backend it was: the server, the
    /// device's identity there, every key and token in the secret store and
    /// the vault header go. The sources, collections and marks stay, and the
    /// next account this device creates or joins takes them along.
    ///
    /// The "remember this device" tokens of UwULock two-step login go too:
    /// disconnecting is where someone expects that to end.
    pub fn disconnect(&self) -> Result<()> {
        let lock = self.lock_state()?;
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        forget_uwusync(&tx)?;
        tx.execute(
            "UPDATE lock_state SET active = 0, signed_in_ms = NULL WHERE id = 1",
            [],
        )?;
        tx.execute("UPDATE meta SET vault_flavor = NULL WHERE id = 1", [])?;
        tx.execute("DELETE FROM vault", [])?;
        tx.commit()?;
        drop(conn);
        if let (Some(server), Some(email)) = (&lock.server_url, &lock.email) {
            self.secrets.delete(&names::lock_remember(server, email));
        }
        for name in [
            names::DEVICE_KEY,
            names::ACCOUNT_KEY,
            names::LOCK_REFRESH_TOKEN,
        ] {
            self.secrets.delete(name);
        }
        self.drop_vault();
        tracing::info!("this device no longer syncs");
        Ok(())
    }

    pub fn set_sync_cursor(&self, cursor: u64) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE sync_state SET cursor = ?1, last_sync_ms = ?2 WHERE id = 1",
            params![cursor as i64, now_ms() as i64],
        )?;
        Ok(())
    }

    /// How many records are waiting to be pushed.
    pub fn pending_count(&self) -> Result<usize> {
        let conn = self.conn.lock();
        let mut total = 0i64;
        for kind in SYNCED_KINDS {
            let Some(table) = table_of(kind) else {
                continue;
            };
            total += conn.query_row(
                &format!("SELECT count(*) FROM {table} WHERE dirty = 1"),
                [],
                |row| row.get::<_, i64>(0),
            )?;
        }
        Ok(total as usize)
    }

    /// Records changed here and not yet accepted by the server, sealed and
    /// ready to push. Needs the vault key, since that is what seals them.
    ///
    /// Returns how many records were **left out** as well: one too large for
    /// the protocol, or whose payload cannot be built, stays where it is
    /// rather than stopping everything else from syncing.
    pub fn pending_envelopes(&self, limit: usize) -> Result<(Vec<Envelope>, usize)> {
        let conn = self.conn.lock();
        let guard = self.vault.lock();
        let vault = guard.as_ref().ok_or(StoreError::VaultLocked)?;
        let vault_id = vault.vault_id();

        let mut envelopes = Vec::new();
        let mut bytes = 0;
        let mut left_out = 0;
        'kinds: for kind in SYNCED_KINDS {
            if envelopes.len() >= limit {
                break;
            }
            let (rows, unreadable) = pending_rows(&conn, kind, limit - envelopes.len())?;
            left_out += unreadable;
            for row in rows {
                let sealed = vault.seal_synced(
                    row.id,
                    row.kind,
                    row.updated_at,
                    row.deleted,
                    &row.payload,
                )?;
                if sealed.blob.len() > MAX_BLOB_BYTES {
                    tracing::warn!(
                        id = %row.id,
                        kind = ?row.kind,
                        bytes = sealed.blob.len(),
                        "record too large for the protocol, left out of the sync"
                    );
                    left_out += 1;
                    continue;
                }
                // The rest waits for the next request: it is still pending.
                if !envelopes.is_empty() && bytes + sealed.blob.len() > MAX_BATCH_BYTES {
                    break 'kinds;
                }
                bytes += sealed.blob.len();
                envelopes.push(Envelope {
                    id: row.id,
                    vault_id,
                    kind: row.kind,
                    updated_at: row.updated_at,
                    base_seq: row.base_seq,
                    deleted: row.deleted,
                    nonce: sealed.nonce,
                    blob: sealed.blob,
                    seq: None,
                });
            }
        }
        Ok((envelopes, left_out))
    }

    /// This device's manifest, sealed, when a newer one waits to be pushed.
    ///
    /// Kept apart from [`Store::pending_envelopes`] on purpose: a server that
    /// does not know the kind yet refuses the whole request it arrives in, and
    /// that must not hold up the records themselves.
    pub fn pending_manifest(&self) -> Result<Option<Envelope>> {
        let conn = self.conn.lock();
        let guard = self.vault.lock();
        let vault = guard.as_ref().ok_or(StoreError::VaultLocked)?;
        let (rows, _) = pending_rows(&conn, EntityKind::Manifest, 1)?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let sealed =
            vault.seal_synced(row.id, row.kind, row.updated_at, row.deleted, &row.payload)?;
        if sealed.blob.len() > MAX_BLOB_BYTES {
            tracing::warn!(
                bytes = sealed.blob.len(),
                "manifest too large, not published"
            );
            return Ok(None);
        }
        Ok(Some(Envelope {
            id: row.id,
            vault_id: vault.vault_id(),
            kind: row.kind,
            updated_at: row.updated_at,
            base_seq: row.base_seq,
            deleted: row.deleted,
            nonce: sealed.nonce,
            blob: sealed.blob,
            seq: None,
        }))
    }

    /// Clear the pending flag for records the server took, and remember the
    /// version it gave them. A row that changed while the push was in flight
    /// keeps its flag.
    pub fn mark_pushed(&self, pushed: &[Pushed]) -> Result<usize> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut cleared = 0;
        for record in pushed {
            let Some(table) = table_of(record.kind) else {
                continue;
            };
            cleared += tx.execute(
                &format!(
                    "UPDATE {table}
                        SET dirty = 0, server_seq = ?2
                      WHERE id = ?1 AND hlc_wall_ms = ?3 AND hlc_counter = ?4 AND hlc_device = ?5"
                ),
                params![
                    record.id.to_string(),
                    record.seq as i64,
                    record.updated_at.wall_ms as i64,
                    record.updated_at.counter,
                    record.updated_at.device,
                ],
            )?;
        }
        tx.commit()?;
        Ok(cleared)
    }

    /// Apply records pulled from the server, in one transaction; the cursor
    /// moves separately, once the batch is in.
    pub fn apply_envelopes(&self, envelopes: &[Envelope]) -> Result<ApplyReport> {
        if envelopes.is_empty() {
            return Ok(ApplyReport::default());
        }
        let mut conn = self.conn.lock();
        let guard = self.vault.lock();
        let vault = guard.as_ref().ok_or(StoreError::VaultLocked)?;

        let mut order: Vec<&Envelope> = envelopes.iter().collect();
        order.sort_by_key(|env| (env.kind.apply_rank(), env.seq.unwrap_or(0)));

        let tx = conn.transaction()?;
        let mut report = ApplyReport::default();
        let mut newest = None::<Hlc>;
        for env in order {
            let outcome = apply_one(&tx, vault, env)?;
            if env.kind == EntityKind::Manifest && outcome.rejected == 0 {
                report.manifests += 1;
            } else {
                report.add(outcome);
            }
            if outcome.rejected == 0 {
                newest = Some(match newest {
                    Some(seen) if seen > env.updated_at => seen,
                    _ => env.updated_at,
                });
            }
        }
        // The local clock catches up with what arrived, capped against a
        // device whose own clock is far in the future.
        if let Some(remote) = newest {
            let (wall, counter): (i64, i64) = tx.query_row(
                "SELECT clock_wall_ms, clock_counter FROM meta WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let merged = clock_of(wall, counter, self.device as i64).merge(remote, now_ms());
            tx.execute(
                "UPDATE meta SET clock_wall_ms = ?1, clock_counter = ?2 WHERE id = 1",
                params![merged.wall_ms as i64, merged.counter as i64],
            )?;
        }
        tx.commit()?;
        Ok(report)
    }

    /// Take a vault as this device's own: its id, its key and whose labels it
    /// uses, and — for UwUSync — its header, so the master password can open
    /// it again.
    ///
    /// The same vault again (signing in once more, a new master password)
    /// only replaces key, header and flavor. Another vault takes the records
    /// here along: they are rewritten under its id and wait to be pushed,
    /// since to the account they are new. Rows are plaintext here, so nothing
    /// has to be sealed again.
    pub fn adopt_vault(
        &self,
        vault_id: Uuid,
        key: Zeroizing<[u8; 32]>,
        flavor: Flavor,
        header: Option<&VaultHeader>,
    ) -> Result<()> {
        self.adopt(vault_id, key, flavor, header, |_| Ok(()))
    }

    /// [`Self::adopt_vault`], with one more step in the same transaction.
    pub(crate) fn adopt(
        &self,
        vault_id: Uuid,
        key: Zeroizing<[u8; 32]>,
        flavor: Flavor,
        header: Option<&VaultHeader>,
        also: impl FnOnce(&Transaction) -> Result<()>,
    ) -> Result<()> {
        if header.is_some_and(|h| h.vault_id != vault_id) {
            return Err(StoreError::Invalid {
                field: "vault",
                problem: "the header is another vault's",
            });
        }
        let adopted = UnlockedVault::from_key(vault_id, key, flavor);
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let id = vault_id.to_string();
        let current = crate::vault_id(&tx)?;
        if current != id {
            tx.execute("UPDATE meta SET vault_id = ?1 WHERE id = 1", [&id])?;
            for kind in SYNCED_KINDS {
                let Some(table) = table_of(kind) else {
                    continue;
                };
                tx.execute(
                    &format!("UPDATE {table} SET vault_id = ?1, server_seq = 0"),
                    [&id],
                )?;
                tx.execute(
                    &format!(
                        "UPDATE {table} SET dirty = 1
                          WHERE deleted = 0
                            AND (hlc_wall_ms > 0 OR hlc_counter > 0 OR hlc_device > 0)"
                    ),
                    [],
                )?;
                // A tombstone means nothing to an account that never had the
                // record.
                tx.execute(&format!("DELETE FROM {table} WHERE deleted = 1"), [])?;
            }
            // Manifests speak of the old vault's devices, and the cursor
            // counted a server this account does not share.
            crate::manifest::forget_manifests(&tx)?;
            tx.execute(
                "UPDATE sync_state SET cursor = 0, last_sync_ms = NULL WHERE id = 1",
                [],
            )?;
        }
        tx.execute(
            "UPDATE meta SET vault_flavor = ?1 WHERE id = 1",
            [flavor.as_str()],
        )?;
        match header {
            Some(header) => crate::vault::store_header(&tx, header)?,
            None => {
                tx.execute("DELETE FROM vault", [])?;
            }
        }
        also(&tx)?;
        tx.commit()?;
        drop(conn);
        self.hold_vault(adopted);
        tracing::info!(vault = %vault_id, flavor = flavor.as_str(), "vault adopted");
        Ok(())
    }
}

/// Forget the UwUSync pairing, inside whatever transaction is open.
pub(crate) fn forget_uwusync(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute(
        "UPDATE sync_state
            SET server_url = NULL, account_id = NULL, device_id = NULL,
                tls_fingerprint = NULL, paired_ms = NULL,
                cursor = 0, last_sync_ms = NULL,
                floor_manifest_id = NULL, floor_wall_ms = NULL,
                floor_counter = NULL, floor_device = NULL
          WHERE id = 1",
        [],
    )?;
    // Without a server, nothing is being kept back. The manifests stay: a
    // device that pairs with the same account again still holds them.
    conn.execute("DELETE FROM manifest_violations", [])?;
    Ok(())
}

/// Rows of one kind waiting to be pushed, with their payload built.
fn pending_rows(
    conn: &rusqlite::Connection,
    kind: EntityKind,
    limit: usize,
) -> Result<(Vec<Pending>, usize)> {
    let Some(table) = table_of(kind) else {
        return Ok((Vec::new(), 0));
    };
    let columns = match kind {
        EntityKind::NewsSource => "url, title, site_url, notify, created_at",
        EntityKind::NewsCollection => {
            "name, symbol, color, all_sources, source_ids, rules, notify, sort_index"
        }
        EntityKind::NewsMark => "article, source_id, read, starred, published_at",
        EntityKind::Manifest => "entries",
    };
    let extra = if kind == EntityKind::Manifest {
        "NULL"
    } else {
        "sync_extra"
    };
    let sql = format!(
        "SELECT id, hlc_wall_ms, hlc_counter, hlc_device, deleted, server_seq, {extra}, {columns}
           FROM {table} WHERE dirty = 1 ORDER BY hlc_wall_ms, hlc_counter LIMIT {limit}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;
    let mut pending = Vec::new();
    let mut unreadable = 0;
    while let Some(row) = rows.next()? {
        let id = parse_uuid(&row.get::<_, String>(0)?)?;
        let updated_at = clock_of(row.get(1)?, row.get(2)?, row.get(3)?);
        let deleted: bool = row.get(4)?;
        let base_seq: i64 = row.get(5)?;
        let extra = extra_from(row.get(6)?);
        // A tombstone has no fields worth sealing — only its header, which the
        // seal covers either way.
        let payload = if deleted {
            Ok(Zeroizing::new(Vec::new()))
        } else {
            payload_of(kind, row, extra)
        };
        let payload = match payload {
            Ok(payload) => payload,
            Err(error) => {
                tracing::warn!(%id, kind = ?kind, %error, "a record that cannot be sealed, left out of the sync");
                unreadable += 1;
                continue;
            }
        };
        pending.push(Pending {
            id,
            kind,
            updated_at,
            deleted,
            base_seq: base_seq as u64,
            payload,
        });
    }
    Ok((pending, unreadable))
}

/// The bytes that go into an envelope: the record's fields as JSON.
fn payload_of(
    kind: EntityKind,
    row: &rusqlite::Row<'_>,
    extra: Extra,
) -> Result<Zeroizing<Vec<u8>>> {
    let json = match kind {
        EntityKind::NewsSource => serde_json::to_vec(&NewsSourcePayload {
            url: row.get(7)?,
            title: row.get(8)?,
            site_url: row.get(9)?,
            notify: row.get(10)?,
            created_at: row.get(11)?,
            extra,
        }),
        EntityKind::NewsCollection => {
            let ids: String = row.get(11)?;
            let rules: String = row.get(12)?;
            serde_json::to_vec(&NewsCollectionPayload {
                name: row.get(7)?,
                symbol: row.get(8)?,
                color: row.get(9)?,
                all_sources: row.get(10)?,
                source_ids: serde_json::from_str(&ids).unwrap_or_default(),
                rules: serde_json::from_str::<NewsRulesPayload>(&rules).unwrap_or_default(),
                notify: row.get(13)?,
                sort_index: row.get(14)?,
                extra,
            })
        }
        EntityKind::NewsMark => {
            let source: String = row.get(8)?;
            serde_json::to_vec(&NewsMarkPayload {
                article: row.get(7)?,
                source_id: parse_uuid(&source)?,
                read: row.get(9)?,
                starred: row.get(10)?,
                published_at: row.get(11)?,
                extra,
            })
        }
        EntityKind::Manifest => return Ok(Zeroizing::new(row.get(7)?)),
    };
    json.map(Zeroizing::new).map_err(|_| StoreError::Invalid {
        field: "record",
        problem: "unserialisable",
    })
}

fn count(f: impl FnOnce(&mut ApplyReport)) -> ApplyReport {
    let mut report = ApplyReport::default();
    f(&mut report);
    report
}

fn skipped() -> ApplyReport {
    count(|r| r.skipped = 1)
}

fn rejected() -> ApplyReport {
    count(|r| r.rejected = 1)
}

fn applied() -> ApplyReport {
    count(|r| r.applied = 1)
}

/// Apply one pulled record.
fn apply_one(tx: &Transaction, vault: &UnlockedVault, env: &Envelope) -> Result<ApplyReport> {
    if env.vault_id != vault.vault_id() {
        tracing::warn!(%env.id, "a record from another vault, dropped");
        return Ok(rejected());
    }
    let Some(table) = table_of(env.kind) else {
        return Ok(skipped());
    };

    // The seal covers the whole header, so this is where a flipped tombstone
    // flag, a rewritten clock or a swapped blob is caught.
    let payload = match vault.open_synced(
        env.id,
        env.kind,
        env.updated_at,
        env.deleted,
        &Sealed {
            nonce: env.nonce.clone(),
            blob: env.blob.clone(),
        },
    ) {
        Ok(payload) => payload,
        Err(_) => {
            tracing::warn!(%env.id, kind = ?env.kind, "a record that failed its seal, dropped");
            return Ok(rejected());
        }
    };

    let local: Option<(i64, i64, i64, bool)> = tx
        .query_row(
            &format!(
                "SELECT hlc_wall_ms, hlc_counter, hlc_device, deleted FROM {table} WHERE id = ?1"
            ),
            [env.id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let seq = env.seq.unwrap_or(0) as i64;

    let Some((wall, counter, device, deleted)) = local else {
        if env.deleted {
            // Something we never had was deleted somewhere else.
            return Ok(skipped());
        }
        // A version given up here already — a pruned mark — stays given up.
        let superseded: Option<Hlc> = tx
            .query_row(
                "SELECT hlc_wall_ms, hlc_counter, hlc_device FROM superseded WHERE id = ?1",
                [env.id.to_string()],
                |row| Ok(clock_of(row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        if superseded.is_some_and(|clock| clock >= env.updated_at) {
            return Ok(skipped());
        }
        return insert_record(tx, vault, env, &payload, seq);
    };

    let decision = resolve(
        Version::new(clock_of(wall, counter, device), deleted),
        Version::new(env.updated_at, env.deleted),
    );
    match decision {
        Resolution::Identical => {
            tx.execute(
                &format!("UPDATE {table} SET dirty = 0, server_seq = ?2 WHERE id = ?1"),
                params![env.id.to_string(), seq],
            )?;
            Ok(count(|r| r.identical = 1))
        }
        Resolution::Local => {
            // Ours is newer. Keep it pending, but base the next push on the
            // version the server holds, or it would be refused forever.
            tx.execute(
                &format!("UPDATE {table} SET server_seq = ?2 WHERE id = ?1"),
                params![env.id.to_string(), seq],
            )?;
            Ok(count(|r| r.kept = 1))
        }
        Resolution::Remote if env.deleted => tombstone_record(tx, env, table, seq),
        Resolution::Remote => insert_record(tx, vault, env, &payload, seq),
    }
}

fn tombstone_record(
    tx: &Transaction,
    env: &Envelope,
    table: &str,
    seq: i64,
) -> Result<ApplyReport> {
    let clock = env.updated_at;
    tx.execute(
        &format!(
            "UPDATE {table}
                SET deleted = 1, rev = rev + 1, dirty = 0, server_seq = ?2,
                    hlc_wall_ms = ?3, hlc_counter = ?4, hlc_device = ?5
              WHERE id = ?1"
        ),
        params![
            env.id.to_string(),
            seq,
            clock.wall_ms as i64,
            clock.counter,
            clock.device
        ],
    )?;
    if env.kind == EntityKind::NewsSource {
        // What this device fetched for it goes as it does on the device that
        // deleted it.
        drop_source_cache(tx, env.id)?;
    }
    Ok(applied())
}

/// Write a record, whether it is new here or a newer version of one we have.
fn insert_record(
    tx: &Transaction,
    vault: &UnlockedVault,
    env: &Envelope,
    payload: &[u8],
    seq: i64,
) -> Result<ApplyReport> {
    let vault_id = env.vault_id.to_string();
    let id = env.id.to_string();
    let clock = env.updated_at;
    let raw = payload;
    let Ok(payload) = PayloadOf::read(env.kind, raw) else {
        // A payload this build cannot read at all is left on the server for
        // one that can, rather than failing the whole batch.
        tracing::warn!(%env.id, kind = ?env.kind, "a record this build cannot read, skipped");
        return Ok(skipped());
    };
    match payload {
        PayloadOf::Source(source) => {
            tx.execute(
                "INSERT INTO news_sources
                    (id, vault_id, url, title, site_url, notify, created_at,
                     hlc_wall_ms, hlc_counter, hlc_device, deleted, dirty, server_seq, sync_extra)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, 0, ?11, ?12)
                 ON CONFLICT (id) DO UPDATE SET
                    url = excluded.url, title = excluded.title, site_url = excluded.site_url,
                    notify = excluded.notify, created_at = excluded.created_at,
                    hlc_wall_ms = excluded.hlc_wall_ms, hlc_counter = excluded.hlc_counter,
                    hlc_device = excluded.hlc_device, deleted = 0, dirty = 0,
                    server_seq = excluded.server_seq, sync_extra = excluded.sync_extra,
                    rev = rev + 1",
                params![
                    id,
                    vault_id,
                    source.url,
                    source.title,
                    source.site_url,
                    source.notify,
                    source.created_at,
                    clock.wall_ms as i64,
                    clock.counter,
                    clock.device,
                    seq,
                    extra_to(&source.extra),
                ],
            )?;
        }
        PayloadOf::Collection(collection) => {
            let ids = serde_json::to_string(&collection.source_ids).unwrap_or_else(|_| "[]".into());
            let rules = serde_json::to_string(&collection.rules).unwrap_or_else(|_| "{}".into());
            tx.execute(
                "INSERT INTO news_collections
                    (id, vault_id, name, symbol, color, all_sources, source_ids, rules,
                     notify, sort_index, hlc_wall_ms, hlc_counter, hlc_device,
                     deleted, dirty, server_seq, sync_extra)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 0, 0, ?14, ?15)
                 ON CONFLICT (id) DO UPDATE SET
                    name = excluded.name, symbol = excluded.symbol, color = excluded.color,
                    all_sources = excluded.all_sources, source_ids = excluded.source_ids,
                    rules = excluded.rules, notify = excluded.notify,
                    sort_index = excluded.sort_index,
                    hlc_wall_ms = excluded.hlc_wall_ms, hlc_counter = excluded.hlc_counter,
                    hlc_device = excluded.hlc_device, deleted = 0, dirty = 0,
                    server_seq = excluded.server_seq, sync_extra = excluded.sync_extra,
                    rev = rev + 1",
                params![
                    id,
                    vault_id,
                    collection.name,
                    collection.symbol,
                    collection.color,
                    collection.all_sources,
                    ids,
                    rules,
                    collection.notify,
                    collection.sort_index,
                    clock.wall_ms as i64,
                    clock.counter,
                    clock.device,
                    seq,
                    extra_to(&collection.extra),
                ],
            )?;
        }
        PayloadOf::Mark(mark) => {
            // The id is the article's: a mark under any other would be a
            // second mark for the same article.
            if env.id != mark_id(&mark.article) {
                tracing::warn!(%env.id, "a mark under another article's id, skipped");
                return Ok(skipped());
            }
            tx.execute(
                "INSERT INTO news_marks
                    (id, vault_id, article, source_id, read, starred, published_at,
                     hlc_wall_ms, hlc_counter, hlc_device, deleted, dirty, server_seq, sync_extra)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, 0, ?11, ?12)
                 ON CONFLICT (id) DO UPDATE SET
                    article = excluded.article, source_id = excluded.source_id,
                    read = excluded.read, starred = excluded.starred,
                    published_at = excluded.published_at,
                    hlc_wall_ms = excluded.hlc_wall_ms, hlc_counter = excluded.hlc_counter,
                    hlc_device = excluded.hlc_device, deleted = 0, dirty = 0,
                    server_seq = excluded.server_seq, sync_extra = excluded.sync_extra,
                    rev = rev + 1",
                params![
                    id,
                    vault_id,
                    mark.article,
                    mark.source_id.to_string(),
                    mark.read,
                    mark.starred,
                    mark.published_at,
                    clock.wall_ms as i64,
                    clock.counter,
                    clock.device,
                    seq,
                    extra_to(&mark.extra),
                ],
            )?;
        }
        PayloadOf::Manifest(manifest) => {
            // The id says whose it is. One that names another device than the
            // one it lists for would let a manifest stand in for another.
            if env.id != crate::manifest::manifest_id(vault.flavor(), env.vault_id, manifest.device)
            {
                tracing::warn!(%env.id, "a manifest under someone else's id, dropped");
                return Ok(rejected());
            }
            tx.execute(
                "INSERT INTO manifests
                    (id, vault_id, entries, hlc_wall_ms, hlc_counter, hlc_device,
                     deleted, dirty, server_seq)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, ?7)
                 ON CONFLICT (id) DO UPDATE SET
                    entries = excluded.entries,
                    hlc_wall_ms = excluded.hlc_wall_ms, hlc_counter = excluded.hlc_counter,
                    hlc_device = excluded.hlc_device, deleted = 0, dirty = 0,
                    server_seq = excluded.server_seq, rev = rev + 1",
                params![
                    id,
                    vault_id,
                    raw,
                    clock.wall_ms as i64,
                    clock.counter,
                    clock.device,
                    seq,
                ],
            )?;
        }
    }
    Ok(applied())
}

enum PayloadOf {
    Source(NewsSourcePayload),
    Collection(NewsCollectionPayload),
    Mark(NewsMarkPayload),
    Manifest(uwunews_proto::Manifest),
}

impl PayloadOf {
    fn read(kind: EntityKind, payload: &[u8]) -> std::result::Result<Self, ()> {
        Ok(match kind {
            EntityKind::NewsSource => Self::Source(serde_json::from_slice(payload).map_err(drop)?),
            EntityKind::NewsCollection => {
                Self::Collection(serde_json::from_slice(payload).map_err(drop)?)
            }
            EntityKind::NewsMark => Self::Mark(serde_json::from_slice(payload).map_err(drop)?),
            // A manifest from a newer build, in a format this one cannot read,
            // is passed over rather than checked against wrongly.
            EntityKind::Manifest => {
                Self::Manifest(uwunews_proto::Manifest::decode(payload).ok_or(())?)
            }
        })
    }
}

/// Remember that a version of a record was given up here without a
/// tombstone. Only the newest such version per id is kept.
pub(crate) fn supersede(tx: &Transaction, kind: EntityKind, id: Uuid, clock: Hlc) -> Result<()> {
    tx.execute(
        "INSERT INTO superseded (id, kind, hlc_wall_ms, hlc_counter, hlc_device)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (id) DO UPDATE SET
            hlc_wall_ms = excluded.hlc_wall_ms, hlc_counter = excluded.hlc_counter,
            hlc_device = excluded.hlc_device
          WHERE (excluded.hlc_wall_ms, excluded.hlc_counter, excluded.hlc_device)
              > (hlc_wall_ms, hlc_counter, hlc_device)",
        params![
            id.to_string(),
            kind as u8,
            clock.wall_ms as i64,
            clock.counter,
            clock.device
        ],
    )?;
    Ok(())
}
