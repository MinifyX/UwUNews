//! Where the secrets go: the device key, the account key, the vault key and
//! the UwULock tokens never reach SQLite. The app hands the store a
//! [`SecretStore`] — the Keychain on Apple platforms — and the store keeps
//! them only through it.

use parking_lot::Mutex;
use std::collections::HashMap;

/// A small keyed store for secrets, provided by the platform.
///
/// Infallible on purpose, as the app's bindings are: an implementation that
/// cannot write logs it and behaves as if the value were absent, which makes
/// the store ask for the password again rather than fail.
pub trait SecretStore: Send + Sync {
    fn get(&self, name: &str) -> Option<Vec<u8>>;
    fn set(&self, name: &str, value: &[u8]);
    fn delete(&self, name: &str);
}

/// The names the store uses.
pub mod names {
    /// The Ed25519 seed this device signs UwUSync's challenges with.
    pub const DEVICE_KEY: &str = "sync.device_key";
    /// The UwUSync account key (the "recovery kit").
    pub const ACCOUNT_KEY: &str = "sync.account_key";
    /// The vault key — on UwULock, the space key.
    pub const VAULT_KEY: &str = "vault.key";
    /// The UwULock refresh token.
    pub const LOCK_REFRESH_TOKEN: &str = "lock.refresh_token";

    /// A UwULock "remember this device" token, for one server and account
    /// only: `lock.remember.<hex sha256(server|email)>`.
    pub fn lock_remember(server_url: &str, email: &str) -> String {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(format!("{server_url}|{email}").as_bytes());
        format!("lock.remember.{}", crate::hex(&digest))
    }
}

/// A [`SecretStore`] in memory: for tests, and for a build without a
/// platform keychain.
#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, Vec<u8>>>);

impl MemorySecrets {
    pub fn new() -> Self {
        Self::default()
    }

    /// The names held right now, sorted.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.0.lock().keys().cloned().collect();
        names.sort();
        names
    }
}

impl SecretStore for MemorySecrets {
    fn get(&self, name: &str) -> Option<Vec<u8>> {
        self.0.lock().get(name).cloned()
    }

    fn set(&self, name: &str, value: &[u8]) {
        self.0.lock().insert(name.to_string(), value.to_vec());
    }

    fn delete(&self, name: &str) {
        self.0.lock().remove(name);
    }
}
