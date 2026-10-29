use crate::common::{share, source, store, Server};
use crate::reading::draft;
use uwunews_proto::{EntityKind, Hlc, NewsMarkPayload};
use uwunews_store::{article_key, CollectionDraft, Flavor};

#[test]
fn records_travel_between_two_devices_and_come_back_identical() {
    let (a, _) = store();
    let (b, _) = store();
    share(&a, &b, Flavor::Ssh);
    let feed = source(&a, "https://a.example.com/feed");
    a.add_collection(CollectionDraft {
        name: "All".into(),
        all_sources: true,
        ..Default::default()
    })
    .unwrap();
    a.upsert_articles(feed.id, &[draft("1", Some(1))]).unwrap();
    a.set_starred(&article_key(feed.id, "1"), true).unwrap();

    let mut server = Server::default();
    let report = server.relay(&a, &b);
    assert_eq!((report.applied, report.rejected), (3, 0));
    assert_eq!(a.pending_count().unwrap(), 0);
    assert_eq!(b.list_sources().unwrap(), a.list_sources().unwrap());
    assert_eq!(b.list_collections().unwrap(), a.list_collections().unwrap());

    // B fetches the same item and sees it starred.
    b.upsert_articles(feed.id, &[draft("1", Some(1))]).unwrap();
    let article = b.article(&article_key(feed.id, "1")).unwrap().unwrap();
    assert!(article.starred);

    // Pulling one's own records back changes nothing.
    let envelopes = b.pending_envelopes(10).unwrap().0;
    assert!(envelopes.is_empty());
}

#[test]
fn the_newer_write_wins_and_a_source_tombstone_drops_the_cache() {
    let (a, _) = store();
    let (b, _) = store();
    share(&a, &b, Flavor::Rdp);
    let feed = source(&a, "https://a.example.com/feed");
    let mut server = Server::default();
    server.relay(&a, &b);
    b.upsert_articles(feed.id, &[draft("1", Some(1))]).unwrap();

    a.update_source(feed.id, Some("Old".into()), false).unwrap();
    // Twice: within one millisecond, a single write of B's would tie with
    // A's and the device ids would decide.
    b.update_source(feed.id, Some("Mid".into()), false).unwrap();
    b.update_source(feed.id, Some("New".into()), false).unwrap();
    let older = server.push(&a);
    let report = b.apply_envelopes(&older).unwrap();
    assert_eq!(report.kept, 1);
    let report = server.relay(&b, &a);
    assert_eq!(report.applied, 1);
    assert_eq!(
        a.source(feed.id).unwrap().unwrap().title.as_deref(),
        Some("New")
    );

    a.delete_source(feed.id).unwrap();
    server.relay(&a, &b);
    assert!(b.list_sources().unwrap().is_empty());
    assert!(b.articles(&Default::default()).unwrap().is_empty());
}

#[test]
fn a_tampered_or_foreign_record_is_rejected() {
    let (a, _) = store();
    let (b, _) = store();
    share(&a, &b, Flavor::Ssh);
    let (c, _) = store();
    let (d, _) = store();
    share(&c, &d, Flavor::Ssh);
    source(&a, "https://a.example.com/feed");
    source(&c, "https://c.example.com/feed");
    let mut server = Server::default();

    let mut flipped = server.push(&a);
    flipped[0].deleted = true;
    flipped[0].updated_at = Hlc::new(u64::MAX / 4, 0, 1);
    assert_eq!(b.apply_envelopes(&flipped).unwrap().rejected, 1);
    let foreign = server.push(&c);
    assert_eq!(b.apply_envelopes(&foreign).unwrap().rejected, 1);
    assert!(b.list_sources().unwrap().is_empty());
}

#[test]
fn unknown_fields_survive_a_round_trip_through_an_older_build() {
    let (a, _) = store();
    let (b, _) = store();
    share(&a, &b, Flavor::Ssh);
    let feed = source(&a, "https://a.example.com/feed");
    let key = article_key(feed.id, "1");
    a.upsert_articles(feed.id, &[draft("1", Some(1))]).unwrap();
    a.set_read(&key, true).unwrap();
    let mut server = Server::default();
    let mut envelopes = server.push(&a);
    let mark = envelopes
        .iter_mut()
        .find(|e| e.kind == EntityKind::NewsMark)
        .unwrap();

    // A newer build added a field: re-seal the mark with it, as a vault
    // holding the same key would.
    let (header, _) = (b.vault_header().unwrap().unwrap(), ());
    let vault = header.unlock_with(b"correct horse", None).unwrap();
    let opened = vault
        .open_synced(
            mark.id,
            mark.kind,
            mark.updated_at,
            mark.deleted,
            &uwunews_vault::Sealed {
                nonce: mark.nonce.clone(),
                blob: mark.blob.clone(),
            },
        )
        .unwrap();
    let mut json: serde_json::Value = serde_json::from_slice(&opened).unwrap();
    json["futureField"] = serde_json::json!({"x": 1});
    let sealed = vault
        .seal_synced(
            mark.id,
            mark.kind,
            mark.updated_at,
            false,
            &serde_json::to_vec(&json).unwrap(),
        )
        .unwrap();
    mark.nonce = sealed.nonce;
    mark.blob = sealed.blob;
    b.apply_envelopes(&envelopes).unwrap();

    // B stars it, and what it pushes still carries the field.
    b.set_starred(&key, true).unwrap();
    let out = server.push(&b);
    let mark = out.iter().find(|e| e.kind == EntityKind::NewsMark).unwrap();
    let opened = vault
        .open_synced(
            mark.id,
            mark.kind,
            mark.updated_at,
            mark.deleted,
            &uwunews_vault::Sealed {
                nonce: mark.nonce.clone(),
                blob: mark.blob.clone(),
            },
        )
        .unwrap();
    let payload: NewsMarkPayload = serde_json::from_slice(&opened).unwrap();
    assert!(payload.read && payload.starred);
    assert!(payload.extra.contains_key("futureField"));
}

#[test]
fn a_mark_under_another_articles_id_is_skipped() {
    let (a, _) = store();
    let (b, _) = store();
    share(&a, &b, Flavor::Ssh);
    let feed = source(&a, "https://a.example.com/feed");
    a.upsert_articles(feed.id, &[draft("1", Some(1)), draft("2", Some(2))])
        .unwrap();
    a.set_read(&article_key(feed.id, "1"), true).unwrap();
    a.set_read(&article_key(feed.id, "2"), true).unwrap();
    let mut server = Server::default();
    let envelopes = server.push(&a);
    let marks: Vec<_> = envelopes
        .iter()
        .filter(|e| e.kind == EntityKind::NewsMark)
        .collect();
    // The second mark's sealed payload, presented under the first one's id:
    // the seal binds the id, so this is a rejection already; re-sealing it
    // under the wrong id is what a buggy peer would do.
    let vault = b
        .vault_header()
        .unwrap()
        .unwrap()
        .unlock_with(b"correct horse", None)
        .unwrap();
    let payload = vault
        .open_synced(
            marks[1].id,
            marks[1].kind,
            marks[1].updated_at,
            false,
            &uwunews_vault::Sealed {
                nonce: marks[1].nonce.clone(),
                blob: marks[1].blob.clone(),
            },
        )
        .unwrap();
    let mut wrong = marks[0].clone();
    let sealed = vault
        .seal_synced(wrong.id, wrong.kind, wrong.updated_at, false, &payload)
        .unwrap();
    wrong.nonce = sealed.nonce;
    wrong.blob = sealed.blob;
    let report = b.apply_envelopes(&[wrong]).unwrap();
    assert_eq!(report.skipped, 1);
}

#[test]
fn a_pruned_mark_is_not_pulled_back_in() {
    let (a, _) = store();
    let (b, _) = store();
    share(&a, &b, Flavor::Ssh);
    let feed = source(&a, "https://a.example.com/feed");
    a.upsert_articles(feed.id, &[draft("1", Some(1))]).unwrap();
    a.set_read(&article_key(feed.id, "1"), true).unwrap();
    let mut server = Server::default();
    let envelopes = server.push(&a);
    let report = a.prune(1_000).unwrap();
    assert_eq!((report.articles, report.marks), (1, 1));
    let report = a.apply_envelopes(&envelopes).unwrap();
    assert_eq!(report.skipped, 1);
    // B never pruned and takes it.
    assert_eq!(b.apply_envelopes(&envelopes).unwrap().applied, 2);
}

#[test]
fn adopting_another_vault_takes_the_records_along() {
    let (a, _) = store();
    let (old, _) = store();
    share(&a, &old, Flavor::Rdp);
    let feed = source(&a, "https://a.example.com/feed");
    let (b, _) = store();
    let (c, _) = store();
    let vault_id = share(&b, &c, Flavor::Ssh);
    let mut server = Server::default();
    server.push(&a);
    assert_eq!(a.pending_count().unwrap(), 0);

    let header = b.vault_header().unwrap().unwrap();
    let unlocked = header.unlock_with(b"correct horse", None).unwrap();
    a.adopt_vault(vault_id, unlocked.export_key(), Flavor::Ssh, Some(&header))
        .unwrap();
    assert_eq!(a.current_vault_id().unwrap(), vault_id);
    assert_eq!(a.vault_flavor(), Some(Flavor::Ssh));
    assert_eq!(a.pending_count().unwrap(), 1);
    server.relay(&a, &b);
    assert_eq!(b.source(feed.id).unwrap().unwrap().url, feed.url);
}
