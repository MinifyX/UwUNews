//! The client handle, as far as it goes without a server: what it says about
//! this device, and what it refuses.

use crate::common::{share, source, store};
use uuid::Uuid;
use uwunews_store::{Enrolment, LockEnrolment, Space, VaultStatus};
use uwunews_sync::client::{Backend, ClientError, SyncClient};
use uwunews_vault::Flavor;
use zeroize::Zeroizing;

#[test]
fn a_new_device_does_not_sync_and_says_so() {
    let client = SyncClient::new(store(), "Test device");
    let status = client.status().unwrap();
    assert_eq!(status.backend, Backend::None);
    assert_eq!(status.vault, VaultStatus::Absent);
    assert!(matches!(client.sync_once(), Err(ClientError::NotConnected)));
    assert!(matches!(
        client.offer_pairing(),
        Err(ClientError::NotConnected)
    ));
    assert!(matches!(
        client.unlock("pw"),
        Err(ClientError::NotConnected)
    ));
    assert!(matches!(client.lock_connection(), Ok(None)));
}

#[test]
fn codes_that_are_not_codes_go_nowhere() {
    let client = SyncClient::new(store(), "Test device");
    assert!(matches!(
        client.create_account("nonsense", "pw"),
        Err(ClientError::BadCode)
    ));
    assert!(matches!(
        client.join("nonsense", None, "pw", Flavor::Ssh),
        Err(ClientError::BadCode)
    ));
    assert!(matches!(
        client.join("K7M4Q-tiger-radio-kiwi", None, "pw", Flavor::Ssh),
        Err(ClientError::NeedsServer)
    ));
}

#[test]
fn a_paired_device_reports_its_account_and_disconnect_keeps_the_data() {
    let store = store();
    share(&[&store], Flavor::Ssh);
    let account_id = Uuid::now_v7();
    store
        .save_enrolment(
            &Enrolment {
                server_url: "https://sync.example.com".into(),
                account_id,
                device_id: Uuid::now_v7(),
                tls_fingerprint: None,
            },
            &[7; 32],
            None,
        )
        .unwrap();
    let feed = source(&store, "https://news.example.com/feed.xml");
    let client = SyncClient::new(store.clone(), "Test device");

    let status = client.status().unwrap();
    assert_eq!(
        status.backend,
        Backend::UwuSync {
            server_url: "https://sync.example.com".into(),
            account_id
        }
    );
    assert_eq!(status.vault, VaultStatus::Unlocked);
    assert_eq!(status.pending, 1);

    client.disconnect().unwrap();
    assert_eq!(client.status().unwrap().backend, Backend::None);
    assert!(matches!(client.sync_once(), Err(ClientError::NotConnected)));
    assert_eq!(store.list_sources().unwrap(), vec![feed]);
}

fn join_lock(store: &uwunews_store::Store, refresh: Option<&str>) -> Uuid {
    let space = Uuid::now_v7();
    store
        .join_space(
            Space {
                id: space,
                key: Zeroizing::new([3; 32]),
            },
            &LockEnrolment {
                server_url: "https://lock.example.com",
                email: "Reader@Example.com",
                kdf: "test",
                refresh_token: refresh,
                remember_token: Some("remember-me"),
            },
        )
        .unwrap();
    space
}

#[test]
fn a_uwulock_device_reports_its_account_and_a_lost_session_asks_for_a_sign_in() {
    let store = store();
    join_lock(&store, Some("refresh-1"));
    let client = SyncClient::new(store.clone(), "Test device");
    assert_eq!(
        client.status().unwrap().backend,
        Backend::UwuLock {
            server_url: "https://lock.example.com".into(),
            email: "Reader@Example.com".into(),
            signed_in: true,
        }
    );
    // The master password is UwULock's business, not this device's.
    assert!(matches!(client.unlock("pw"), Err(ClientError::SignInAgain)));
    assert!(matches!(
        client.offer_pairing(),
        Err(ClientError::NotConnected)
    ));

    store.end_lock_session().unwrap();
    let status = client.status().unwrap();
    assert!(matches!(
        status.backend,
        Backend::UwuLock {
            signed_in: false,
            ..
        }
    ));
    assert!(matches!(client.sync_once(), Err(ClientError::SignInAgain)));
    // Only the issuing account sees its remember token.
    assert!(store
        .lock_remember_token("https://lock.example.com", "reader@example.com")
        .is_some());
    assert!(store
        .lock_remember_token("https://other.example.com", "reader@example.com")
        .is_none());
}

#[test]
fn moving_to_uwulock_forgets_uwusync_and_keeps_the_sources() {
    let store = store();
    share(&[&store], Flavor::Ssh);
    store
        .save_enrolment(
            &Enrolment {
                server_url: "https://sync.example.com".into(),
                account_id: Uuid::now_v7(),
                device_id: Uuid::now_v7(),
                tls_fingerprint: Some("SHA256:abc".into()),
            },
            &[7; 32],
            None,
        )
        .unwrap();
    let feed = source(&store, "https://news.example.com/feed.xml");
    let space = join_lock(&store, None);

    let client = SyncClient::new(store.clone(), "Test device");
    assert!(matches!(
        client.status().unwrap().backend,
        Backend::UwuLock {
            signed_in: false,
            ..
        }
    ));
    assert!(store.enrolment_keys().unwrap().is_none());
    assert_eq!(store.current_vault_id().unwrap(), space);
    assert_eq!(store.list_sources().unwrap(), vec![feed]);
    assert_eq!(client.status().unwrap().pending, 1);
}
