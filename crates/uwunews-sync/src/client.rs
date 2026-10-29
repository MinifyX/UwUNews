//! The whole of sync behind one handle, for the app: which backend this device
//! uses, getting onto one, and a pass.
//!
//! Blocking, every call: the app runs it on a thread of its own. It holds the
//! store and, once made, the connection to the server; everything worth
//! keeping lives in the store and its secret store, so a new [`SyncClient`]
//! after a restart picks up where the old one was.
//!
//! One backend at a time. Creating or joining a UwUSync account stops a
//! UwULock sign-in from being used, and signing in to UwULock forgets a
//! UwUSync pairing; either way the sources, collections and marks here come
//! along into the new account.

use crate::engine::{sync_once, SyncError, SyncReport, TransportError};
use crate::flow::{self, FlowError, PairingOffer, Setup};
use crate::http::Server;
use crate::lock::{
    self, AcceptSpace, Lock, LockDevice, LockError, SignIn, SignInOutcome, TwoFactorAnswer,
    TwoFactorMethod,
};
use crate::pairing::{self, Joined, Target};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;
use uwunews_store::{LockEnrolment, Store, StoreError, VaultStatus};
use uwunews_vault::{AccountKey, Flavor, VaultError};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("this device does not sync")]
    NotConnected,
    #[error("that code is not one")]
    BadCode,
    /// A spoken pairing code says nothing about the server: it needs the
    /// address, or the pasteable code.
    #[error("a spoken code needs the server's address")]
    NeedsServer,
    /// The vault key is not here: the master password brings it back
    /// ([`SyncClient::unlock`]), or for UwULock a sign-in.
    #[error("the vault has to be unlocked first")]
    VaultLocked,
    /// The UwULock session is over: sign in again with the master password.
    #[error("sign in to UwULock again")]
    SignInAgain,
    #[error(transparent)]
    Flow(#[from] FlowError),
    #[error(transparent)]
    Lock(#[from] LockError),
    #[error(transparent)]
    Sync(#[from] SyncError),
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Vault(#[from] VaultError),
}

type Result<T> = std::result::Result<T, ClientError>;

/// Which backend this device syncs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    None,
    UwuSync {
        server_url: String,
        account_id: Uuid,
    },
    UwuLock {
        server_url: String,
        email: String,
        /// A refresh token is kept: syncing needs no master password.
        signed_in: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub backend: Backend,
    pub vault: VaultStatus,
    /// Local changes waiting to go up.
    pub pending: usize,
    pub last_sync_ms: Option<u64>,
}

/// What one pass did, in the three numbers the app shows. The whole report
/// is in `report`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Synced {
    pub pushed: usize,
    pub pulled: usize,
    pub conflicts: usize,
    pub report: SyncReport,
}

/// How a UwULock sign-in went.
pub enum LockOutcome {
    /// Signed in; `made_space` when the account had no UwUNews data yet.
    SignedIn { made_space: bool },
    /// The account has two-step login: ask for a code and sign in again.
    TwoFactor {
        methods: Vec<TwoFactorMethod>,
        message: Option<String>,
    },
    /// The account's space is not the one this device used there. Nothing
    /// changed; ask, and sign in again with `accept_space` if they agree.
    SpaceChanged { was: Uuid, now: Option<Uuid> },
}

/// A UwULock sign-in, as the form gives it.
pub struct LockLogin<'a> {
    pub server_url: &'a str,
    pub email: &'a str,
    pub password: &'a str,
    pub two_factor: Option<TwoFactorAnswer>,
    pub accept_space: Option<AcceptSpace>,
}

enum Connection {
    UwuSync(Arc<Server>),
    UwuLock(Arc<Lock>),
}

pub struct SyncClient {
    store: Arc<Store>,
    device_name: String,
    connection: Mutex<Option<Connection>>,
}

impl SyncClient {
    /// `device_name` is what the other devices list this one as.
    pub fn new(store: Arc<Store>, device_name: impl Into<String>) -> Self {
        Self {
            store,
            device_name: device_name.into(),
            connection: Mutex::new(None),
        }
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub fn status(&self) -> Result<Status> {
        let sync = self.store.sync_state()?;
        let lock = self.store.lock_state()?;
        let backend = match (&sync.server_url, sync.account_id) {
            (Some(url), Some(account_id)) if sync.paired() => Backend::UwuSync {
                server_url: url.clone(),
                account_id,
            },
            _ if lock.active => Backend::UwuLock {
                server_url: lock.server_url.clone().unwrap_or_default(),
                email: lock.email.clone().unwrap_or_default(),
                signed_in: lock.signed_in,
            },
            _ => Backend::None,
        };
        Ok(Status {
            backend,
            vault: self.store.vault_status()?,
            pending: self.store.pending_count()?,
            last_sync_ms: sync.last_sync_ms,
        })
    }

    // ── UwUSync ─────────────────────────────────────────────────────────────

    /// Make a UwUSync account from a server's setup code (`uwu1_…`), with this
    /// device as its first. Returns the recovery code of the account key, to
    /// be shown once.
    pub fn create_account(&self, setup_code: &str, password: &str) -> Result<String> {
        let setup = flow::parse_setup(setup_code).ok_or(ClientError::BadCode)?;
        self.create_account_with(&setup, password)
    }

    pub fn create_account_with(&self, setup: &Setup, password: &str) -> Result<String> {
        let (server, paired) =
            flow::create_account(&self.store, setup, password.as_bytes(), &self.device_name)?;
        *self.connection.lock() = Some(Connection::UwuSync(Arc::new(server)));
        let key = paired.recovery.ok_or(FlowError::NoAccountKey)?;
        Ok(key.to_code(Flavor::Ssh))
    }

    /// Join a UwUSync account from the code another device shows: the
    /// pasteable `uwu2_…` form, or the spoken one with the server's address.
    /// `flavor` is the app that made the account — UwUNews and UwUSSH make
    /// [`Flavor::Ssh`] accounts, UwURDP [`Flavor::Rdp`] ones.
    pub fn join(
        &self,
        code: &str,
        server: Option<(&str, Option<&str>)>,
        password: &str,
        flavor: Flavor,
    ) -> Result<()> {
        let target = target_of(code, server)?;
        let deadline = Instant::now() + pairing::DEADLINE;
        let (server, _) = flow::join(
            &self.store,
            &target,
            password.as_bytes(),
            &self.device_name,
            flavor,
            deadline,
        )?;
        *self.connection.lock() = Some(Connection::UwuSync(Arc::new(server)));
        Ok(())
    }

    /// Open a pairing session for another device. Show `offer.offer.spoken`
    /// or `.pasteable`, then call [`Self::wait_for_device`].
    pub fn offer_pairing(&self) -> Result<PairingOffer> {
        let server = self.uwusync()?;
        Ok(flow::offer_pairing(&self.store, &server)?)
    }

    /// Ends an offered pairing early: its code stops working, and a
    /// [`Self::wait_for_device`] still waiting on it gives up.
    pub fn close_pairing(&self, id: &str) -> Result<()> {
        let server = self.uwusync()?;
        server.close_pairing(id)?;
        Ok(())
    }

    pub fn wait_for_device(&self, offer: &PairingOffer, timeout: Duration) -> Result<Joined> {
        let server = self.uwusync()?;
        let deadline = Instant::now() + timeout.min(pairing::DEADLINE);
        Ok(flow::wait_for_device(
            &self.store,
            &server,
            offer,
            deadline,
        )?)
    }

    /// The master password, for a UwUSync device whose secret store lost the
    /// vault key. A UwULock device signs in again instead.
    pub fn unlock(&self, password: &str) -> Result<()> {
        if !self.store.sync_state()?.paired() {
            return Err(if self.store.lock_state()?.active {
                ClientError::SignInAgain
            } else {
                ClientError::NotConnected
            });
        }
        let account_key: Option<AccountKey> = flow::account_key(&self.store)?;
        self.store
            .unlock_vault(password.as_bytes(), account_key.as_ref())?;
        Ok(())
    }

    /// The connection to the backend, made now if there is none yet: a sign-in
    /// to UwUSync with the device key, or a UwULock session from the kept
    /// refresh token.
    pub fn reconnect(&self) -> Result<()> {
        self.connection.lock().take();
        self.connect().map(|_| ())
    }

    // ── UwULock ─────────────────────────────────────────────────────────────

    /// Sign in to a UwULock account and sync its `news` space from now on.
    /// For a device that does not sync yet, one whose session ended, and one
    /// that moves over from UwUSync.
    pub fn connect_lock(&self, login: &LockLogin<'_>) -> Result<LockOutcome> {
        let server = lock::normalize_server(login.server_url).ok();
        let remembered = match &login.two_factor {
            None => lock::remember_token(&self.store, login.server_url, login.email)?,
            Some(_) => None,
        };
        let request = SignIn {
            server_url: login.server_url,
            email: login.email,
            password: login.password,
            two_factor: login.two_factor.clone(),
            remember_token: remembered.as_deref().map(String::as_str),
            known: lock::known(&self.store, login.server_url, login.email)?,
            accept_space: login.accept_space,
        };
        let signed = match lock::sign_in(&request, self.device()?)? {
            SignInOutcome::SignedIn(signed) => *signed,
            SignInOutcome::TwoFactor { methods, message } => {
                return Ok(LockOutcome::TwoFactor { methods, message })
            }
            SignInOutcome::SpaceChanged {
                was,
                now,
                remember_token,
            } => {
                if let (Some(token), Some(server)) = (remember_token, &server) {
                    self.store
                        .keep_lock_remember_token(server, login.email, &token);
                }
                return Ok(LockOutcome::SpaceChanged { was, now });
            }
        };
        let kdf = signed.kdf_to_keep();
        self.store.join_space(
            uwunews_store::Space {
                id: signed.space.id,
                key: signed.space.key.clone(),
            },
            &LockEnrolment {
                server_url: &signed.server_url,
                email: &signed.email,
                kdf: &kdf,
                refresh_token: signed.refresh_token.as_deref().map(String::as_str),
                remember_token: signed.remember_token.as_deref().map(String::as_str),
            },
        )?;
        let made_space = signed.made_space;
        let lock = signed.lock.keeping(keeper(&self.store));
        *self.connection.lock() = Some(Connection::UwuLock(Arc::new(lock)));
        Ok(LockOutcome::SignedIn { made_space })
    }

    /// Ask for a two-step code by email.
    pub fn send_lock_email_code(
        &self,
        server_url: &str,
        email: &str,
        password: &str,
    ) -> Result<()> {
        let known = lock::known(&self.store, server_url, email)?;
        lock::send_email_code(server_url, email, password, &known, self.device()?)?;
        Ok(())
    }

    /// The UwULock connection, for the realtime channel ([`lock::live::run`]).
    pub fn lock_connection(&self) -> Result<Option<Arc<Lock>>> {
        Ok(match self.connect() {
            Ok(Connection::UwuLock(lock)) => Some(lock),
            Ok(Connection::UwuSync(_)) | Err(ClientError::NotConnected) => None,
            Err(error) => return Err(error),
        })
    }

    // ── Both ────────────────────────────────────────────────────────────────

    /// One pass: push, pull, and the manifests.
    pub fn sync_once(&self) -> Result<Synced> {
        if self.store.vault_status()? == VaultStatus::Locked {
            return Err(if self.store.lock_state()?.active {
                ClientError::SignInAgain
            } else {
                ClientError::VaultLocked
            });
        }
        let report = match self.connect()? {
            Connection::UwuSync(server) => sync_once(&self.store, server.as_ref()),
            Connection::UwuLock(lock) => sync_once(&self.store, lock.as_ref()),
        };
        match report {
            Ok(report) => Ok(Synced {
                pushed: report.pushed,
                pulled: report.pulled,
                conflicts: report.conflicts,
                report,
            }),
            Err(SyncError::Transport(TransportError::SignIn(reason))) => {
                self.connection.lock().take();
                if self.store.lock_state()?.active {
                    tracing::info!(%reason, "the UwULock session ended");
                    self.store.end_lock_session()?;
                    Err(ClientError::SignInAgain)
                } else {
                    Err(TransportError::SignIn(reason).into())
                }
            }
            Err(SyncError::VaultLocked) => Err(ClientError::VaultLocked),
            Err(error) => Err(error.into()),
        }
    }

    /// Stop syncing, whichever backend: every key and token goes, the
    /// sources, collections and marks stay.
    pub fn disconnect(&self) -> Result<()> {
        self.connection.lock().take();
        self.store.disconnect()?;
        Ok(())
    }

    fn device(&self) -> Result<LockDevice> {
        Ok(LockDevice::this_system(
            self.store.lock_device_identifier()?,
        ))
    }

    fn uwusync(&self) -> Result<Arc<Server>> {
        if !self.store.sync_state()?.paired() {
            return Err(ClientError::NotConnected);
        }
        match self.connect()? {
            Connection::UwuSync(server) => Ok(server),
            Connection::UwuLock(_) => Err(ClientError::NotConnected),
        }
    }

    /// The cached connection, or one made from what the store keeps.
    fn connect(&self) -> Result<Connection> {
        let mut cached = self.connection.lock();
        if let Some(connection) = cached.as_ref() {
            return Ok(connection.clone());
        }
        let connection = if self.store.sync_state()?.paired() {
            Connection::UwuSync(Arc::new(flow::reconnect(&self.store)?))
        } else {
            let state = self.store.lock_state()?;
            let (true, Some(url), Some(space)) = (state.active, state.server_url, state.space_id)
            else {
                return Err(ClientError::NotConnected);
            };
            let refresh = self
                .store
                .lock_refresh_token()
                .ok_or(ClientError::SignInAgain)?;
            Connection::UwuLock(Arc::new(
                Lock::connect(&url, self.device()?)?
                    .with_space(space)
                    .with_refresh_token(refresh)
                    .keeping(keeper(&self.store)),
            ))
        };
        *cached = Some(connection.clone());
        Ok(connection)
    }
}

impl Clone for Connection {
    fn clone(&self) -> Self {
        match self {
            Self::UwuSync(server) => Self::UwuSync(Arc::clone(server)),
            Self::UwuLock(lock) => Self::UwuLock(Arc::clone(lock)),
        }
    }
}

/// Keep a new refresh token as soon as the server hands it out.
fn keeper(store: &Arc<Store>) -> impl Fn(&str) + Send + Sync + 'static {
    let store = Arc::clone(store);
    move |token: &str| {
        if let Err(error) = store.keep_lock_refresh_token(token) {
            tracing::warn!(%error, "could not keep the new UwULock refresh token");
        }
    }
}

/// A pairing code, pasted or spoken.
fn target_of(code: &str, server: Option<(&str, Option<&str>)>) -> Result<Target> {
    if let Some(target) = pairing::parse_offer(code) {
        return Ok(target);
    }
    let (id, words) = pairing::parse_spoken(code).ok_or(ClientError::BadCode)?;
    let (server_url, fingerprint) = server.ok_or(ClientError::NeedsServer)?;
    Ok(Target {
        server_url: server_url.trim().to_string(),
        tls_fingerprint: fingerprint.map(str::to_string),
        id,
        words,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_code_carries_its_server_and_a_spoken_one_needs_it() {
        let offer = pairing::offer(
            "K7M4Q",
            "tiger-radio-kiwi",
            "https://sync.example.com",
            None,
        );
        let target = target_of(&offer.pasteable, None).unwrap();
        assert_eq!(target.server_url, "https://sync.example.com");

        assert!(matches!(
            target_of("K7M4Q-tiger-radio-kiwi", None),
            Err(ClientError::NeedsServer)
        ));
        let target = target_of(
            "k7m4q tiger radio kiwi",
            Some(("https://sync.example.com", Some("SHA256:abc"))),
        )
        .unwrap();
        assert_eq!(
            (target.id.as_str(), target.words.as_str()),
            ("K7M4Q", "tiger-radio-kiwi")
        );
        assert_eq!(target.tls_fingerprint.as_deref(), Some("SHA256:abc"));

        assert!(matches!(
            target_of("hello", None),
            Err(ClientError::BadCode)
        ));
    }
}
