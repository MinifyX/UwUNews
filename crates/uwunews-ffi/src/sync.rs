//! Syncing, as the settings screen drives it: a UwUSync account made here or
//! joined from another device, or a UwULock sign-in.

use crate::{invalid, Engine, NewsError, Result};
use std::time::Duration;
use uuid::Uuid;
use uwunews_store::{Flavor, VaultStatus};
use uwunews_sync::client::{Backend, ClientError, LockLogin, LockOutcome};
use uwunews_sync::lock::{AcceptSpace, TwoFactorAnswer};

impl From<ClientError> for NewsError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Store(error) => error.into(),
            ClientError::Transport(_) => NewsError::Network {
                message: error.to_string(),
            },
            other => NewsError::Sync {
                message: other.to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum SyncBackend {
    /// Not syncing.
    Off,
    UwuSync {
        server_url: String,
        account_id: String,
    },
    UwuLock {
        server_url: String,
        email: String,
        signed_in: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum VaultState {
    Absent,
    Locked,
    Unlocked,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SyncStatus {
    pub backend: SyncBackend,
    pub vault: VaultState,
    /// Local changes waiting to go up.
    pub pending: u32,
    pub last_sync_at: Option<i64>,
}

/// The app that made a UwUSync account; its keys are derived its way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AccountFlavor {
    /// UwUNews or UwUSSH.
    Ssh,
    /// UwURDP.
    Rdp,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct PairingCode {
    /// The id and three words, to read out.
    pub spoken: String,
    /// One string with everything, to paste.
    pub pasteable: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SyncSummary {
    pub pushed: u32,
    pub pulled: u32,
    pub conflicts: u32,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct LockSignIn {
    pub server_url: String,
    pub email: String,
    pub password: String,
    /// The second step: the provider asked for and the code typed.
    pub two_factor_provider: Option<u8>,
    pub two_factor_code: Option<String>,
    pub remember: bool,
    /// After [`LockResult::SpaceChanged`]: the space to take, its id or
    /// `"new"`.
    pub accept_space: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct TwoFactorOption {
    pub provider: u8,
    /// `authenticator`, `email`, `yubikey`, `duo`, `webauthn`, …
    pub kind: String,
    /// Whether a code typed here can do it.
    pub supported: bool,
    /// For email codes: where the code goes, masked.
    pub hint: Option<String>,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum LockResult {
    SignedIn {
        made_space: bool,
    },
    TwoFactor {
        options: Vec<TwoFactorOption>,
        message: Option<String>,
    },
    /// The account's space is not the one used here before: ask, then sign
    /// in again with `accept_space`.
    SpaceChanged {
        was: String,
        now: Option<String>,
    },
}

#[uniffi::export]
impl Engine {
    pub fn sync_status(&self) -> Result<SyncStatus> {
        let status = self.sync.status()?;
        Ok(SyncStatus {
            backend: match status.backend {
                Backend::None => SyncBackend::Off,
                Backend::UwuSync {
                    server_url,
                    account_id,
                } => SyncBackend::UwuSync {
                    server_url,
                    account_id: account_id.to_string(),
                },
                Backend::UwuLock {
                    server_url,
                    email,
                    signed_in,
                } => SyncBackend::UwuLock {
                    server_url,
                    email,
                    signed_in,
                },
            },
            vault: match status.vault {
                VaultStatus::Absent => VaultState::Absent,
                VaultStatus::Locked => VaultState::Locked,
                VaultStatus::Unlocked => VaultState::Unlocked,
            },
            pending: status.pending as u32,
            last_sync_at: status.last_sync_ms.map(|ms| ms as i64),
        })
    }

    /// Makes a UwUSync account from a server's setup code; the recovery code
    /// comes back, to be written down.
    pub fn create_sync_account(&self, setup_code: String, password: String) -> Result<String> {
        Ok(self.sync.create_account(setup_code.trim(), &password)?)
    }

    /// Joins a UwUSync account another device shows a code for: the pasted
    /// form, or the spoken one with the server's address.
    pub fn join_sync(
        &self,
        code: String,
        server_url: Option<String>,
        fingerprint: Option<String>,
        password: String,
        flavor: AccountFlavor,
    ) -> Result<()> {
        let server_url = server_url.filter(|s| !s.trim().is_empty());
        let fingerprint = fingerprint.filter(|s| !s.trim().is_empty());
        let server = server_url
            .as_deref()
            .map(|url| (url.trim(), fingerprint.as_deref().map(str::trim)));
        let flavor = match flavor {
            AccountFlavor::Ssh => Flavor::Ssh,
            AccountFlavor::Rdp => Flavor::Rdp,
        };
        self.sync.join(code.trim(), server, &password, flavor)?;
        Ok(())
    }

    /// Opens a pairing for another device; then [`Self::wait_for_device`].
    pub fn offer_pairing(&self) -> Result<PairingCode> {
        let offer = self.sync.offer_pairing()?;
        let code = PairingCode {
            spoken: offer.offer.spoken.clone(),
            pasteable: offer.offer.pasteable.clone(),
        };
        *self.open_pairing.lock() = Some(offer.offer.id.clone());
        *self.pairing.lock() = Some(offer);
        Ok(code)
    }

    /// Waits for the device the last offer was for; its name comes back.
    pub fn wait_for_device(&self, timeout_secs: u32) -> Result<String> {
        let offer = self
            .pairing
            .lock()
            .take()
            .ok_or_else(|| invalid("no pairing is open"))?;
        let joined = self
            .sync
            .wait_for_device(&offer, Duration::from_secs(u64::from(timeout_secs)));
        let cancelled = self.open_pairing.lock().take().is_none();
        match joined {
            Err(_) if cancelled => Err(NewsError::Sync {
                message: "pairing cancelled".into(),
            }),
            joined => Ok(joined?.device_name),
        }
    }

    /// Withdraws the offered code: nobody can join with it any more.
    pub fn cancel_pairing(&self) -> Result<()> {
        *self.pairing.lock() = None;
        match self.open_pairing.lock().take() {
            Some(id) => Ok(self.sync.close_pairing(&id)?),
            None => Ok(()),
        }
    }

    /// The master password, when this device lost its vault key.
    pub fn unlock_sync(&self, password: String) -> Result<()> {
        Ok(self.sync.unlock(&password)?)
    }

    pub fn connect_lock(&self, sign_in: LockSignIn) -> Result<LockResult> {
        let two_factor = match (sign_in.two_factor_provider, sign_in.two_factor_code) {
            (Some(provider), Some(code)) if !code.trim().is_empty() => Some(TwoFactorAnswer {
                provider,
                code: code.trim().replace(' ', ""),
                remember: sign_in.remember,
            }),
            _ => None,
        };
        let accept_space = match sign_in.accept_space.as_deref() {
            None => None,
            Some("new") => Some(AcceptSpace::New),
            Some(id) => Some(AcceptSpace::Listed(
                Uuid::parse_str(id).map_err(|_| invalid("not a space"))?,
            )),
        };
        let outcome = self.sync.connect_lock(&LockLogin {
            server_url: sign_in.server_url.trim(),
            email: sign_in.email.trim(),
            password: &sign_in.password,
            two_factor,
            accept_space,
        })?;
        Ok(match outcome {
            LockOutcome::SignedIn { made_space } => LockResult::SignedIn { made_space },
            LockOutcome::TwoFactor { methods, message } => LockResult::TwoFactor {
                options: methods
                    .into_iter()
                    .map(|m| TwoFactorOption {
                        provider: m.provider,
                        kind: m.kind.to_string(),
                        supported: m.supported,
                        hint: m.hint,
                    })
                    .collect(),
                message,
            },
            LockOutcome::SpaceChanged { was, now } => LockResult::SpaceChanged {
                was: was.to_string(),
                now: now.map(|id| id.to_string()),
            },
        })
    }

    pub fn send_lock_email_code(
        &self,
        server_url: String,
        email: String,
        password: String,
    ) -> Result<()> {
        Ok(self
            .sync
            .send_lock_email_code(server_url.trim(), email.trim(), &password)?)
    }

    /// One pass up and down. Not connected is an error; the app checks
    /// [`Self::sync_status`] first.
    pub fn sync_now(&self) -> Result<SyncSummary> {
        let _one = self.syncing.lock();
        let synced = self.sync.sync_once()?;
        Ok(SyncSummary {
            pushed: synced.pushed as u32,
            pulled: synced.pulled as u32,
            conflicts: synced.conflicts as u32,
        })
    }

    /// Stops syncing; everything here stays.
    pub fn disconnect_sync(&self) -> Result<()> {
        let _ = self.cancel_pairing();
        Ok(self.sync.disconnect()?)
    }
}
