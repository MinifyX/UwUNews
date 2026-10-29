//! Syncing through a UwULock Server instead of UwUSync: what this device
//! keeps of it.
//!
//! On UwULock the vault's key is the key of the account's `news` space, and
//! the vault's id is the space's id — so the records are sealed exactly as
//! they are for UwUSync, and only the key comes from somewhere else: the
//! UwULock account, opened with its master password. Joining a space is
//! therefore [`Store::adopt_vault`] with that key. There is no vault header
//! in this mode: the key lives in the secret store, and a device that lost it
//! signs in again.
//!
//! What is kept besides: the server, the email and the space here; the
//! refresh token and the "remember this device" token of two-step login in
//! the secret store. The remember token is kept per account — server and
//! email — and only handed out for the account that issued it: another server
//! must never see it. The master password and the keys above the space key
//! are never kept.

use crate::secret::names;
use crate::sync::forget_uwusync;
use crate::{now_ms, parse_uuid, Result, Store, StoreError};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::Serialize;
use uuid::Uuid;
use uwunews_vault::Flavor;
use zeroize::Zeroizing;

/// Where this device stands with UwULock. Local, never synced.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LockState {
    /// UwULock is the backend: the vault is the account's space.
    pub active: bool,
    pub server_url: Option<String>,
    pub email: Option<String>,
    pub space_id: Option<Uuid>,
    /// A refresh token is kept: syncing needs no master password.
    pub signed_in: bool,
    pub signed_in_ms: Option<u64>,
}

/// A space this device takes as its vault.
pub struct Space {
    pub id: Uuid,
    pub key: Zeroizing<[u8; 32]>,
}

impl std::fmt::Debug for Space {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Space({}, key redacted)", self.id)
    }
}

/// What this device learnt about one UwULock account at earlier sign-ins.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LockAccount {
    /// The key derivation the last sign-in used, as `uwunews_sync` wrote it.
    pub kdf: Option<String>,
    /// The space this device used on the account.
    pub space_id: Option<Uuid>,
    /// The spaces it used before and moved on from, oldest first.
    pub left_spaces: Vec<Uuid>,
}

/// A sign-in, on its way into the store.
pub struct LockEnrolment<'a> {
    /// Normalized, as `uwunews_sync::lock::normalize_server` makes it.
    pub server_url: &'a str,
    pub email: &'a str,
    /// The key derivation this sign-in used, as `uwunews_sync` writes it.
    pub kdf: &'a str,
    pub refresh_token: Option<&'a str>,
    /// A new "remember this device" token for this account; `None` keeps the
    /// one it has.
    pub remember_token: Option<&'a str>,
}

/// An account's email as it is kept: trimmed and in lower case, as UwULock
/// (and Bitwarden) compare it.
fn account_email(email: &str) -> String {
    email.trim().to_lowercase()
}

fn remember_name(server_url: &str, email: &str) -> String {
    names::lock_remember(server_url, &account_email(email))
}

impl Store {
    pub fn lock_state(&self) -> Result<LockState> {
        let conn = self.conn.lock();
        let mut state = conn.query_row(
            "SELECT active, server_url, email, space_id, signed_in_ms
               FROM lock_state WHERE id = 1",
            [],
            |row| {
                let space: Option<String> = row.get(3)?;
                let signed_in_ms: Option<i64> = row.get(4)?;
                Ok(LockState {
                    active: row.get(0)?,
                    server_url: row.get(1)?,
                    email: row.get(2)?,
                    space_id: space.and_then(|id| Uuid::parse_str(&id).ok()),
                    signed_in: false,
                    signed_in_ms: signed_in_ms.map(|ms| ms as u64),
                })
            },
        )?;
        drop(conn);
        state.signed_in = state.active && self.secrets.get(names::LOCK_REFRESH_TOKEN).is_some();
        Ok(state)
    }

    /// What this device learnt about an account at earlier sign-ins: the
    /// server (normalized, as it is kept) and the email.
    pub fn lock_account(&self, server_url: &str, email: &str) -> Result<LockAccount> {
        let row: Option<(Option<String>, Option<String>, String)> = self
            .conn
            .lock()
            .query_row(
                "SELECT kdf, space_id, left_spaces FROM lock_accounts
                  WHERE server_url = ?1 AND email = ?2",
                params![server_url, account_email(email)],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((kdf, space_id, left_spaces)) = row else {
            return Ok(LockAccount::default());
        };
        Ok(LockAccount {
            kdf,
            space_id: space_id.as_deref().map(parse_uuid).transpose()?,
            left_spaces: left_spaces_of(&left_spaces)?,
        })
    }

    /// The id this install logs in to UwULock with: made once, kept for good,
    /// so the server lists one device however often it signs in.
    pub fn lock_device_identifier(&self) -> Result<Uuid> {
        let text: String = self.conn.lock().query_row(
            "SELECT device_identifier FROM lock_state WHERE id = 1",
            [],
            |row| row.get(0),
        )?;
        parse_uuid(&text)
    }

    /// Take a space as this device's vault and UwULock as its backend, in one
    /// transaction. A UwUSync pairing this device had is forgotten: one
    /// backend at a time.
    ///
    /// The same space again (signing in once more) only replaces the key;
    /// any other one takes the records here along, as
    /// [`Store::adopt_vault`] does.
    pub fn join_space(&self, space: Space, enrolment: &LockEnrolment<'_>) -> Result<()> {
        let space_id = space.id;
        let remember = |tx: &Transaction| -> Result<()> {
            tx.execute(
                "UPDATE lock_state
                    SET active = 1, server_url = ?1, email = ?2, space_id = ?3,
                        signed_in_ms = ?4
                  WHERE id = 1",
                params![
                    enrolment.server_url,
                    enrolment.email,
                    space_id.to_string(),
                    now_ms() as i64,
                ],
            )?;
            let email = account_email(enrolment.email);
            // The space this device used on the account before, if it was
            // another, is one it has left: never to be taken again.
            let before: Option<(Option<String>, String)> = tx
                .query_row(
                    "SELECT space_id, left_spaces FROM lock_accounts
                      WHERE server_url = ?1 AND email = ?2",
                    params![enrolment.server_url, email],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let mut left = match &before {
                Some((_, left)) => left_spaces_of(left)?,
                None => Vec::new(),
            };
            if let Some(old) = before.and_then(|(old, _)| old) {
                let old = parse_uuid(&old)?;
                if old != space_id && !left.contains(&old) {
                    left.push(old);
                }
            }
            left.retain(|id| *id != space_id);
            tx.execute(
                "INSERT INTO lock_accounts (server_url, email, kdf, space_id, left_spaces)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (server_url, email) DO UPDATE
                    SET kdf = excluded.kdf, space_id = excluded.space_id,
                        left_spaces = excluded.left_spaces",
                params![
                    enrolment.server_url,
                    email,
                    enrolment.kdf,
                    space_id.to_string(),
                    serde_json::to_string(&left).map_err(|_| StoreError::Invalid {
                        field: "left_spaces",
                        problem: "unserialisable",
                    })?,
                ],
            )?;
            forget_uwusync(tx)?;
            Ok(())
        };
        // A space key is UwUNews' own: the labels are the default ones.
        self.adopt(space.id, space.key, Flavor::Ssh, None, remember)?;
        self.secrets.delete(names::DEVICE_KEY);
        self.secrets.delete(names::ACCOUNT_KEY);
        match enrolment.refresh_token {
            Some(token) => self
                .secrets
                .set(names::LOCK_REFRESH_TOKEN, token.as_bytes()),
            None => self.secrets.delete(names::LOCK_REFRESH_TOKEN),
        }
        if let Some(token) = enrolment.remember_token {
            self.keep_lock_remember_token(enrolment.server_url, enrolment.email, token);
        }
        tracing::info!(space = %space_id, "this device syncs through UwULock");
        Ok(())
    }

    /// A new refresh token: servers hand one out with every refresh, and the
    /// old one stops working. Kept before it is used, or a crash in between
    /// would cost the sign-in.
    pub fn keep_lock_refresh_token(&self, token: &str) -> Result<()> {
        if self.lock_state()?.active {
            self.secrets
                .set(names::LOCK_REFRESH_TOKEN, token.as_bytes());
        }
        Ok(())
    }

    /// The refresh token, when one is kept.
    pub fn lock_refresh_token(&self) -> Option<Zeroizing<String>> {
        token_of(self.secrets.get(names::LOCK_REFRESH_TOKEN))
    }

    /// The "remember this device" token of two-step login that this server
    /// (normalized, as it is kept) issued for this email, and only that one.
    pub fn lock_remember_token(&self, server_url: &str, email: &str) -> Option<Zeroizing<String>> {
        token_of(self.secrets.get(&remember_name(server_url, email)))
    }

    /// Keep a "remember this device" token for this account: also for a
    /// sign-in that got one but did not get as far as [`Store::join_space`].
    pub fn keep_lock_remember_token(&self, server_url: &str, email: &str, token: &str) {
        self.secrets
            .set(&remember_name(server_url, email), token.as_bytes());
    }

    /// The session is over — signed out elsewhere, the device removed, the
    /// refresh token run out: the next sign-in needs the master password.
    /// UwULock stays the backend, and everything here stays as it is.
    pub fn end_lock_session(&self) -> Result<()> {
        self.secrets.delete(names::LOCK_REFRESH_TOKEN);
        tracing::info!("the UwULock session ended");
        Ok(())
    }
}

fn token_of(bytes: Option<Vec<u8>>) -> Option<Zeroizing<String>> {
    let bytes = Zeroizing::new(bytes?);
    std::str::from_utf8(&bytes)
        .ok()
        .map(|text| Zeroizing::new(text.to_string()))
}

fn left_spaces_of(text: &str) -> Result<Vec<Uuid>> {
    serde_json::from_str(text).map_err(|_| StoreError::Invalid {
        field: "left_spaces",
        problem: "not a list of ids",
    })
}
