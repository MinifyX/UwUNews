//! Accounts other apps made: a UwURDP account seals with its own labels, and
//! a device that joins it must know.

use crate::common::{pass, share, source, store, vault};
use uwunews_sync::{sync_once, MemoryServer};
use uwunews_vault::Flavor;

#[test]
fn an_rdp_account_syncs_like_any_other() {
    let (a, b) = (store(), store());
    share(&[&a, &b], Flavor::Rdp);
    let server = MemoryServer::new();
    let feed = source(&a, "https://news.example.com/feed.xml");
    pass(&a, &server);
    pass(&b, &server);
    assert_eq!(b.list_sources().unwrap(), vec![feed]);
    assert_eq!(b.vault_flavor(), Some(Flavor::Rdp));
}

#[test]
fn the_wrong_flavor_opens_nothing() {
    let (a, b) = (store(), store());
    let (header, unlocked) = vault(Flavor::Rdp);
    a.adopt_vault(
        unlocked.vault_id(),
        unlocked.export_key(),
        Flavor::Rdp,
        Some(&header),
    )
    .unwrap();
    // The same key, taken as a UwUSSH account's.
    b.adopt_vault(
        unlocked.vault_id(),
        unlocked.export_key(),
        Flavor::Ssh,
        None,
    )
    .unwrap();
    let server = MemoryServer::new();
    source(&a, "https://news.example.com/feed.xml");
    pass(&a, &server);

    let report = sync_once(&b, &server).unwrap();
    assert!(report.apply.rejected > 0);
    assert!(b.list_sources().unwrap().is_empty());
}
