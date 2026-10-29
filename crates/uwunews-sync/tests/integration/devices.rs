//! Two devices on one account, through the server.

use crate::common::{article, pass, share, source, store};
use uwunews_store::{ArticleQuery, CollectionDraft};
use uwunews_sync::MemoryServer;
use uwunews_vault::Flavor;

#[test]
fn sources_collections_and_marks_reach_the_other_device() {
    let (a, b) = (store(), store());
    share(&[&a, &b], Flavor::Ssh);
    let server = MemoryServer::new();

    let feed = source(&a, "https://news.example.com/feed.xml");
    a.add_collection(CollectionDraft {
        name: "Morning".into(),
        symbol: "sun.max".into(),
        color: "orange".into(),
        source_ids: vec![feed.id],
        ..Default::default()
    })
    .unwrap();
    let key = article(&a, feed.id, "one");
    a.set_starred(&key, true).unwrap();
    let up = pass(&a, &server);
    assert_eq!(up.pushed, 3);
    assert_eq!(a.pending_count().unwrap(), 0);

    let down = pass(&b, &server);
    assert_eq!(down.pulled, 3);
    assert!(down.complete && !down.withheld.any());
    assert_eq!(b.list_sources().unwrap(), vec![feed.clone()]);
    let collections = b.list_collections().unwrap();
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0].source_ids, vec![feed.id]);

    // The mark waits for the article: B's own fetch of the feed finds it.
    article(&b, feed.id, "one");
    let starred = b
        .articles(&ArticleQuery {
            starred_only: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(starred.len(), 1);
    assert_eq!(starred[0].key, key);
}

#[test]
fn a_concurrent_edit_is_a_conflict_and_both_settle_on_the_newer() {
    let (a, b) = (store(), store());
    share(&[&a, &b], Flavor::Ssh);
    let server = MemoryServer::new();
    let feed = source(&a, "https://news.example.com/feed.xml");
    pass(&a, &server);
    pass(&b, &server);

    a.update_source(feed.id, Some("From A".into()), false)
        .unwrap();
    // Twice, so B's write is the newer even within one millisecond.
    b.update_source(feed.id, Some("Draft".into()), true)
        .unwrap();
    b.update_source(feed.id, Some("From B".into()), true)
        .unwrap();
    pass(&a, &server);
    let report = pass(&b, &server);
    assert_eq!(report.conflicts, 1);
    pass(&a, &server);

    let on_a = a.source(feed.id).unwrap().unwrap();
    let on_b = b.source(feed.id).unwrap().unwrap();
    assert_eq!(on_a, on_b);
    assert_eq!(on_a.title.as_deref(), Some("From B"));
    assert_eq!(
        (a.pending_count().unwrap(), b.pending_count().unwrap()),
        (0, 0)
    );
}

#[test]
fn a_deleted_source_goes_everywhere() {
    let (a, b) = (store(), store());
    share(&[&a, &b], Flavor::Ssh);
    let server = MemoryServer::new();
    let feed = source(&a, "https://news.example.com/feed.xml");
    let other = source(&a, "https://blog.example.org/atom.xml");
    pass(&a, &server);
    pass(&b, &server);
    assert_eq!(b.list_sources().unwrap().len(), 2);

    b.delete_source(feed.id).unwrap();
    pass(&b, &server);
    pass(&a, &server);
    assert_eq!(a.list_sources().unwrap(), vec![other]);
    // A deleted source cannot come back as it was: adding it again is a new one.
    let again = source(&a, "https://news.example.com/feed.xml");
    assert_ne!(again.id, feed.id);
}

#[test]
fn read_on_one_device_is_read_on_the_other() {
    let (a, b) = (store(), store());
    share(&[&a, &b], Flavor::Ssh);
    let server = MemoryServer::new();
    let feed = source(&a, "https://news.example.com/feed.xml");
    pass(&a, &server);
    pass(&b, &server);
    let key = article(&a, feed.id, "two");
    article(&b, feed.id, "two");

    assert_eq!(a.mark_all_read(None).unwrap(), 1);
    pass(&a, &server);
    pass(&b, &server);
    assert!(b.article(&key).unwrap().unwrap().read);
    assert_eq!(
        b.unread_counts()
            .unwrap()
            .get(&feed.id)
            .copied()
            .unwrap_or(0),
        0
    );
}

#[test]
fn a_device_joining_later_brings_its_own_and_gets_the_rest() {
    let (a, b) = (store(), store());
    let server = MemoryServer::new();
    source(&a, "https://news.example.com/feed.xml");
    source(&b, "https://blog.example.org/atom.xml");
    // B followed a feed before it joined: adopting the vault keeps it.
    share(&[&a, &b], Flavor::Ssh);
    pass(&a, &server);
    pass(&b, &server);
    pass(&a, &server);
    assert_eq!(a.list_sources().unwrap().len(), 2);
    assert_eq!(b.list_sources().unwrap().len(), 2);
}
