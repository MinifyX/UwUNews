use crate::common::{share, store, vault};
use uwunews_store::{
    secret::names, Enrolment, Flavor, LockEnrolment, SecretStore, Space, Store, VaultStatus,
};
use zeroize::Zeroizing;

const SERVER: &str = "https://sync.example.com";

fn enrol(store: &Store) {
    store
        .save_enrolment(
            &Enrolment {
                server_url: SERVER.into(),
                account_id: uuid::Uuid::now_v7(),
                device_id: uuid::Uuid::now_v7(),
                tls_fingerprint: Some("ab".repeat(32)),
            },
            &[7; 32],
            Some(&uwunews_vault::AccountKey::generate()),
        )
        .unwrap();
}

#[test]
fn keys_live_in_the_secret_store_only() {
    let (store, secrets) = store();
    let (other, _) = crate::common::store();
    share(&store, &other, Flavor::Ssh);
    enrol(&store);
    let mut names = secrets.names();
    names.sort();
    assert_eq!(
        names,
        vec![names::ACCOUNT_KEY, names::DEVICE_KEY, names::VAULT_KEY]
    );
    let keys = store.enrolment_keys().unwrap().unwrap();
    assert_eq!(*keys.device_key, [7; 32]);
    assert!(keys.account_key.is_some());
    assert!(store.sync_state().unwrap().paired());
}

#[test]
fn a_reopened_store_unlocks_from_the_secret_store_or_with_the_password() {
    let dir = std::env::temp_dir().join(format!("uwunews-store-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("news.sqlite");
    let secrets = std::sync::Arc::new(uwunews_store::MemorySecrets::new());
    let (header, unlocked) = vault(Flavor::Rdp);
    {
        let store = Store::open(&path, secrets.clone()).unwrap();
        store
            .adopt_vault(
                unlocked.vault_id(),
                unlocked.export_key(),
                Flavor::Rdp,
                Some(&header),
            )
            .unwrap();
    }
    let store = Store::open(&path, secrets.clone()).unwrap();
    assert_eq!(store.vault_status().unwrap(), VaultStatus::Unlocked);
    assert_eq!(store.vault_flavor(), Some(Flavor::Rdp));

    // The keychain lost the key: the master password opens the vault again.
    secrets.delete(names::VAULT_KEY);
    drop(store);
    let store = Store::open(&path, secrets.clone()).unwrap();
    assert_eq!(store.vault_status().unwrap(), VaultStatus::Locked);
    assert!(store.unlock_vault(b"wrong", None).is_err());
    store.unlock_vault(b"correct horse", None).unwrap();
    assert_eq!(store.vault_status().unwrap(), VaultStatus::Unlocked);
    assert!(secrets.get(names::VAULT_KEY).is_some());
    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn disconnect_drops_every_secret_and_keeps_the_records() {
    let (store, secrets) = store();
    let (other, _) = crate::common::store();
    share(&store, &other, Flavor::Ssh);
    enrol(&store);
    crate::common::source(&store, "https://a.example.com/feed");
    store.disconnect().unwrap();
    assert!(secrets.names().is_empty());
    assert!(!store.sync_state().unwrap().paired());
    assert_eq!(store.vault_status().unwrap(), VaultStatus::Absent);
    assert!(store.vault_header().unwrap().is_none());
    assert_eq!(store.list_sources().unwrap().len(), 1);
}

#[test]
fn joining_a_space_replaces_the_uwusync_pairing_and_keeps_tokens_per_account() {
    let (store, secrets) = store();
    let (other, _) = crate::common::store();
    share(&store, &other, Flavor::Ssh);
    enrol(&store);
    let lock = "https://lock.example.com";
    let space = uuid::Uuid::now_v7();
    store
        .join_space(
            Space {
                id: space,
                key: Zeroizing::new([3; 32]),
            },
            &LockEnrolment {
                server_url: lock,
                email: " Someone@Example.com ",
                kdf: "argon2id",
                refresh_token: Some("refresh"),
                remember_token: Some("remember"),
            },
        )
        .unwrap();
    assert!(!store.sync_state().unwrap().paired());
    assert!(store.vault_header().unwrap().is_none());
    let state = store.lock_state().unwrap();
    assert!(state.active && state.signed_in);
    assert_eq!(state.space_id, Some(space));
    assert_eq!(store.current_vault_id().unwrap(), space);
    assert_eq!(
        store
            .lock_remember_token(lock, "someone@example.com")
            .as_deref()
            .map(|s| s.as_str()),
        Some("remember")
    );
    assert!(store
        .lock_remember_token("https://other.example.com", "someone@example.com")
        .is_none());
    assert!(secrets.get(names::DEVICE_KEY).is_none());
    assert_eq!(
        store
            .lock_account(lock, "SOMEONE@example.com")
            .unwrap()
            .space_id,
        Some(space)
    );

    store.end_lock_session().unwrap();
    assert!(!store.lock_state().unwrap().signed_in);
    assert!(store.lock_refresh_token().is_none());
}
