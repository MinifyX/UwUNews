//! Manifests on this device: publishing its own, and checking everyone's.
//!
//! The format and the reasoning are in `uwunews_proto::manifest`. What lives
//! here is what the store decides:
//!
//! - **What a manifest lists.** Rows the server has confirmed and nothing has
//!   changed since — `dirty = 0` and a server version. A row changed here but
//!   not yet pushed is left out: the version the server has of it is no longer
//!   known here, and listing the local one would claim something the server
//!   never saw.
//! - **What counts as missing.** After a pull that reached the end, every
//!   version a manifest lists must be here, or something newer — a later edit,
//!   or a tombstone, which beats every edit anyway. A listed tombstone for a
//!   record this device never had is fine, and so is a mark pruned here (see
//!   `superseded`).
//! - **Other apps in the same vault.** A UwUSSH or UwURDP device's manifest
//!   lists kinds this build has no table for; those entries are not checked,
//!   and those apps likewise skip the news kinds in ours. There is no alarm
//!   across apps — each checks only what it keeps.
//! - **What it means.** Nothing is trusted or refused on the strength of a
//!   check: UwUNews has no host keys. What was found is written down and the
//!   app can say that the server held something back.

use crate::sync::{clock_of, table_of, SYNCED_KINDS};
use crate::{parse_uuid, tick, Result, Store};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use uuid::Uuid;
use uwunews_proto::{EntityKind, Hlc, Manifest, ManifestEntry, MAX_MANIFEST_ENTRIES};
use uwunews_vault::Flavor;

/// The order a manifest lists kinds in, which is also what survives when a
/// vault has more records than one manifest holds: what the person set up
/// first, the marks last.
const PRIORITY: [EntityKind; 3] = [
    EntityKind::NewsSource,
    EntityKind::NewsCollection,
    EntityKind::NewsMark,
];

/// The id of a device's manifest in a vault: the same on every device, so each
/// device has exactly one, and it can be named before it has arrived. A
/// version-8 UUID from SHA-256, so it never meets a record's random id.
///
/// The label is the vault's flavor's (`uwussh/manifest/v1` or
/// `uwurdp/manifest/v1`), so the devices of the app that made the vault
/// accept this one's manifest as well-formed.
pub fn manifest_id(flavor: Flavor, vault_id: Uuid, device: u32) -> Uuid {
    let mut hash = Sha256::new();
    hash.update(flavor.label("manifest/v1"));
    hash.update(vault_id.as_bytes());
    hash.update(device.to_be_bytes());
    let digest = hash.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

/// A manifest a joining device must see before it believes it has everything:
/// the one the device that added it had published, at least that new.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFloor {
    pub id: Uuid,
    pub clock: Hlc,
}

/// What was wrong with one record a manifest listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Problem {
    /// Not here at all.
    Missing,
    /// Here, but only in a version older than the one listed.
    Older,
    /// Expected and not checked yet: the manifest the pairing promised, until
    /// a pull first reaches the end. It raises no alarm — nothing is known to
    /// be missing.
    Unchecked,
    /// A pull that never reached the end: the server kept saying there was
    /// more. Anything could be among what it did not hand over.
    Incomplete,
}

impl Problem {
    fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Older => "older",
            Self::Unchecked => "unchecked",
            Self::Incomplete => "incomplete",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "older" => Self::Older,
            "unchecked" => Self::Unchecked,
            "incomplete" => Self::Incomplete,
            _ => Self::Missing,
        }
    }
}

/// One record the server should have handed over and did not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Violation {
    pub kind: EntityKind,
    pub id: Uuid,
    pub problem: Problem,
}

/// What the last check found, as the page shows it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Withheld {
    /// Records — and manifests — missing or older than they should be.
    pub records: usize,
}

impl Withheld {
    pub fn any(&self) -> bool {
        self.records > 0
    }
}

/// Forget every manifest and what was found against them — for a device that
/// takes over another vault, whose devices these are not.
pub(crate) fn forget_manifests(tx: &Transaction) -> Result<()> {
    tx.execute_batch(
        "DELETE FROM manifests;
         DELETE FROM superseded;
         DELETE FROM manifest_violations;
         UPDATE sync_state
            SET floor_manifest_id = NULL, floor_wall_ms = NULL,
                floor_counter = NULL, floor_device = NULL
          WHERE id = 1;",
    )?;
    Ok(())
}

fn current_vault(tx: &Transaction) -> Result<Uuid> {
    parse_uuid(&crate::vault_id(tx)?)
}

/// A clock as four columns come out of a row, starting at `first`.
fn clock_at(row: &rusqlite::Row<'_>, first: usize) -> rusqlite::Result<Hlc> {
    Ok(clock_of(
        row.get(first)?,
        row.get(first + 1)?,
        row.get(first + 2)?,
    ))
}

impl Store {
    /// The id this device's manifest has in the current vault.
    pub fn own_manifest_id(&self) -> Result<Uuid> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        Ok(manifest_id(self.flavor(), current_vault(&tx)?, self.device))
    }

    /// Write a new manifest for this device if what it holds changed since the
    /// last one. Returns whether there is a new one to push.
    ///
    /// Meant for the end of a sync pass that got everything out and pulled to
    /// the end — that is when what is here is what the server has too.
    pub fn refresh_manifest(&self) -> Result<bool> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let vault = current_vault(&tx)?;
        let id = manifest_id(self.flavor(), vault, self.device);

        let mut entries = Vec::new();
        for kind in PRIORITY {
            let Some(table) = table_of(kind) else {
                continue;
            };
            let mut stmt = tx.prepare(&format!(
                "SELECT id, hlc_wall_ms, hlc_counter, hlc_device, deleted FROM {table}
                  WHERE dirty = 0 AND server_seq > 0
                    AND (hlc_wall_ms > 0 OR hlc_counter > 0 OR hlc_device > 0)
                  ORDER BY id"
            ))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, clock_at(row, 1)?, row.get(4)?))
                })?
                .collect::<rusqlite::Result<Vec<(String, Hlc, bool)>>>()?;
            for (row_id, clock, deleted) in rows {
                entries.push(ManifestEntry::new(
                    kind,
                    parse_uuid(&row_id)?,
                    clock,
                    deleted,
                ));
            }
        }
        let partial = entries.len() > MAX_MANIFEST_ENTRIES;
        if partial {
            tracing::warn!(
                records = entries.len(),
                listed = MAX_MANIFEST_ENTRIES,
                "more records than one manifest lists; the rest go unchecked"
            );
            entries.truncate(MAX_MANIFEST_ENTRIES);
        }

        let previous: Option<Vec<u8>> = tx
            .query_row(
                "SELECT entries FROM manifests WHERE id = ?1 AND deleted = 0",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(previous) = previous.as_deref().and_then(Manifest::decode) {
            if previous.device == self.device
                && previous.partial == partial
                && previous.entries == entries
            {
                return Ok(false);
            }
        }

        let clock = tick(&tx, self.device)?;
        let payload = Manifest {
            device: self.device,
            created_at: clock,
            partial,
            entries,
        }
        .encode();
        // The server's version of the row stays, so the push is based on it.
        tx.execute(
            "INSERT INTO manifests
                (id, vault_id, entries, hlc_wall_ms, hlc_counter, hlc_device, dirty)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)
             ON CONFLICT (id) DO UPDATE SET
                entries = excluded.entries, hlc_wall_ms = excluded.hlc_wall_ms,
                hlc_counter = excluded.hlc_counter, hlc_device = excluded.hlc_device,
                deleted = 0, dirty = 1, rev = rev + 1",
            params![
                id.to_string(),
                vault.to_string(),
                payload,
                clock.wall_ms as i64,
                clock.counter,
                clock.device,
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// This device's manifest as the server last confirmed it — what a device
    /// it adds is told to wait for. `None` while there is none, or while a
    /// newer one has not gone up yet: the device being added could otherwise
    /// wait for one that never arrives.
    pub fn published_manifest(&self) -> Result<Option<ManifestFloor>> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let id = manifest_id(self.flavor(), current_vault(&tx)?, self.device);
        let clock = tx
            .query_row(
                "SELECT hlc_wall_ms, hlc_counter, hlc_device FROM manifests
                  WHERE id = ?1 AND dirty = 0 AND deleted = 0 AND server_seq > 0",
                [id.to_string()],
                |row| clock_at(row, 0),
            )
            .optional()?;
        Ok(clock.map(|clock| ManifestFloor { id, clock }))
    }

    /// The version of a manifest held here, by id.
    pub fn manifest_clock(&self, id: Uuid) -> Result<Option<Hlc>> {
        Ok(self
            .conn
            .lock()
            .query_row(
                "SELECT hlc_wall_ms, hlc_counter, hlc_device FROM manifests
                  WHERE id = ?1 AND deleted = 0",
                [id.to_string()],
                |row| clock_at(row, 0),
            )
            .optional()?)
    }

    /// Remember the manifest the device that added this one had published.
    ///
    /// Until a pull reaches the end and the check finds it, that manifest
    /// counts as not checked yet.
    pub fn set_manifest_floor(&self, floor: Option<ManifestFloor>) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE sync_state
                SET floor_manifest_id = ?1, floor_wall_ms = ?2,
                    floor_counter = ?3, floor_device = ?4
              WHERE id = 1",
            params![
                floor.map(|f| f.id.to_string()),
                floor.map(|f| f.clock.wall_ms as i64),
                floor.map(|f| f.clock.counter),
                floor.map(|f| f.clock.device),
            ],
        )?;
        if let Some(floor) = floor {
            tx.execute(
                "INSERT OR IGNORE INTO manifest_violations (kind, id, problem)
                 VALUES (?1, ?2, ?3)",
                params![
                    EntityKind::Manifest as u8,
                    floor.id.to_string(),
                    Problem::Unchecked.as_str()
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Write down that a pull did not reach the end. Until one does and the
    /// check runs, this counts as something kept back.
    pub fn note_incomplete_pull(&self) -> Result<Withheld> {
        self.conn.lock().execute(
            "INSERT OR REPLACE INTO manifest_violations (kind, id, problem)
             VALUES (?1, ?2, ?3)",
            params![
                EntityKind::Manifest as u8,
                Uuid::nil().to_string(),
                Problem::Incomplete.as_str()
            ],
        )?;
        tracing::warn!("the server never let the pull reach the end");
        self.withheld()
    }

    /// The manifest this device was told to wait for when it joined.
    pub fn manifest_floor(&self) -> Result<Option<ManifestFloor>> {
        let row: Option<(String, Hlc)> = self
            .conn
            .lock()
            .query_row(
                "SELECT floor_manifest_id, floor_wall_ms, floor_counter, floor_device
                   FROM sync_state
                  WHERE id = 1 AND floor_manifest_id IS NOT NULL
                    AND floor_wall_ms IS NOT NULL AND floor_counter IS NOT NULL
                    AND floor_device IS NOT NULL",
                [],
                |row| Ok((row.get(0)?, clock_at(row, 1)?)),
            )
            .optional()?;
        row.map(|(id, clock)| {
            Ok(ManifestFloor {
                id: parse_uuid(&id)?,
                clock,
            })
        })
        .transpose()
    }

    /// Hold every manifest against what is here, write down what is missing,
    /// and say what that means.
    ///
    /// Only meaningful right after a pull that reached the end: before that,
    /// what a manifest lists may simply not have been fetched yet.
    pub fn check_manifests(&self) -> Result<Withheld> {
        let floor = self.manifest_floor()?;
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;

        let manifests = {
            let mut stmt = tx.prepare(
                "SELECT id, entries, hlc_wall_ms, hlc_counter, hlc_device
                   FROM manifests WHERE deleted = 0",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        clock_at(row, 2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };

        let mut found = BTreeMap::<(u8, Uuid), Problem>::new();
        for (_, entries, _) in &manifests {
            let Some(manifest) = Manifest::decode(entries) else {
                continue;
            };
            for entry in &manifest.entries {
                let Some(kind) = entry.kind().filter(|kind| SYNCED_KINDS.contains(kind)) else {
                    continue;
                };
                if let Some(problem) = check_entry(&tx, kind, entry)? {
                    found.insert((kind as u8, entry.id), problem);
                }
            }
        }

        if let Some(floor) = floor {
            let held = manifests
                .iter()
                .find(|(id, _, _)| *id == floor.id.to_string())
                .map(|(_, _, clock)| *clock);
            let problem = match held {
                Some(clock) if clock >= floor.clock => None,
                Some(_) => Some(Problem::Older),
                None => Some(Problem::Missing),
            };
            if let Some(problem) = problem {
                found.insert((EntityKind::Manifest as u8, floor.id), problem);
            }
        }

        tx.execute("DELETE FROM manifest_violations", [])?;
        for ((kind, id), problem) in &found {
            tx.execute(
                "INSERT INTO manifest_violations (kind, id, problem) VALUES (?1, ?2, ?3)",
                params![kind, id.to_string(), problem.as_str()],
            )?;
        }
        tx.commit()?;
        drop(conn);

        let withheld = self.withheld()?;
        if withheld.any() {
            tracing::warn!(
                records = withheld.records,
                "the server did not hand over everything the other devices hold"
            );
        }
        Ok(withheld)
    }

    /// What the last check found. A manifest not checked yet does not count
    /// as a record kept back.
    pub fn withheld(&self) -> Result<Withheld> {
        let records: i64 = self.conn.lock().query_row(
            "SELECT count(*) FROM manifest_violations WHERE problem <> ?1",
            [Problem::Unchecked.as_str()],
            |row| row.get(0),
        )?;
        Ok(Withheld {
            records: records as usize,
        })
    }

    /// Every record the last check found missing or out of date.
    pub fn manifest_violations(&self) -> Result<Vec<Violation>> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT kind, id, problem FROM manifest_violations ORDER BY kind, id")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, u8>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut violations = Vec::new();
        for (kind, id, problem) in rows {
            let Some(kind) = EntityKind::from_discriminant(kind) else {
                continue;
            };
            violations.push(Violation {
                kind,
                id: parse_uuid(&id)?,
                problem: Problem::parse(&problem),
            });
        }
        Ok(violations)
    }
}

/// Whether one listed version is accounted for here, and if not, how.
fn check_entry(
    tx: &Transaction,
    kind: EntityKind,
    entry: &ManifestEntry,
) -> Result<Option<Problem>> {
    let Some(table) = table_of(kind) else {
        return Ok(None);
    };
    let local: Option<(Hlc, bool)> = tx
        .prepare_cached(&format!(
            "SELECT hlc_wall_ms, hlc_counter, hlc_device, deleted FROM {table} WHERE id = ?1"
        ))?
        .query_row([entry.id.to_string()], |row| {
            Ok((clock_at(row, 0)?, row.get(3)?))
        })
        .optional()?;
    match local {
        // A tombstone beats every edit, whenever it was made: the record ends
        // deleted here whatever else there is.
        Some((_, true)) => return Ok(None),
        // The listed version or a later one — a local edit not pushed yet
        // included.
        Some((clock, false)) if clock >= entry.updated_at => return Ok(None),
        // A delete of something this device never had.
        None if entry.deleted => return Ok(None),
        _ => {}
    }
    // A mark pruned here.
    let superseded: Option<Hlc> = tx
        .prepare_cached(
            "SELECT hlc_wall_ms, hlc_counter, hlc_device FROM superseded WHERE id = ?1",
        )?
        .query_row([entry.id.to_string()], |row| clock_at(row, 0))
        .optional()?;
    if superseded.is_some_and(|clock| clock >= entry.updated_at) {
        return Ok(None);
    }
    Ok(Some(if local.is_some() {
        Problem::Older
    } else {
        Problem::Missing
    }))
}

impl Store {
    /// The vault's flavor, or UwUSSH's while no key is held.
    pub(crate) fn flavor(&self) -> Flavor {
        self.vault_flavor().unwrap_or_default()
    }
}
