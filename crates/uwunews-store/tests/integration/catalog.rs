use crate::common::{source, store};
use uwunews_store::{CollectionDraft, CollectionRules, SourceDraft, StoreError};

#[test]
fn sources_are_added_renamed_listed_and_refused_twice() {
    let (store, _) = store();
    let b = source(&store, "https://b.example.com/feed");
    let a = store
        .add_source(SourceDraft {
            url: " https://a.example.com/feed ".into(),
            title: Some("  ".into()),
            site_url: Some("https://a.example.com/".into()),
            notify: true,
        })
        .unwrap();
    assert_eq!(a.url, "https://a.example.com/feed");
    assert_eq!(a.title, None);
    assert!(matches!(
        store.add_source(SourceDraft {
            url: "https://a.example.com/feed".into(),
            ..Default::default()
        }),
        Err(StoreError::DuplicateSource)
    ));

    let renamed = store.update_source(b.id, Some("Zed".into()), true).unwrap();
    assert_eq!(renamed.title.as_deref(), Some("Zed"));
    assert!(renamed.notify);
    let list: Vec<_> = store
        .list_sources()
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(list, vec![a.id, b.id]);
    assert_eq!(store.pending_count().unwrap(), 2);
}

#[test]
fn deleting_a_source_takes_it_out_of_collections_and_drops_its_cache() {
    let (store, _) = store();
    let a = source(&store, "https://a.example.com/feed");
    let b = source(&store, "https://b.example.com/feed");
    let collection = store
        .add_collection(CollectionDraft {
            name: "Both".into(),
            source_ids: vec![a.id, b.id, a.id],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(collection.source_ids, vec![a.id, b.id]);
    store
        .upsert_articles(a.id, &[crate::reading::draft("one", Some(1))])
        .unwrap();

    store.delete_source(a.id).unwrap();
    assert!(store.source(a.id).unwrap().is_none());
    assert!(matches!(
        store.delete_source(a.id),
        Err(StoreError::UnknownSource(_))
    ));
    let collection = &store.list_collections().unwrap()[0];
    assert_eq!(collection.source_ids, vec![b.id]);
    assert!(store.articles(&Default::default()).unwrap().is_empty());
}

#[test]
fn collections_keep_their_order_and_rules() {
    let (store, _) = store();
    let draft = |name: &str| CollectionDraft {
        name: name.into(),
        symbol: "newspaper".into(),
        rules: CollectionRules {
            any_keywords: vec!["rust".into(), " Rust ".into(), "".into()],
            search_in: "title_and_content".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let one = store.add_collection(draft("One")).unwrap();
    let two = store.add_collection(draft("Two")).unwrap();
    let three = store.add_collection(draft("Three")).unwrap();
    assert_eq!(one.rules.any_keywords, vec!["rust"]);
    assert_eq!((one.sort_index, three.sort_index), (0, 2));
    assert!(matches!(
        store.add_collection(draft(" ")),
        Err(StoreError::Invalid { field: "name", .. })
    ));

    store.reorder_collections(&[three.id, one.id]).unwrap();
    let order: Vec<_> = store
        .list_collections()
        .unwrap()
        .into_iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(order, vec![three.id, one.id, two.id]);

    let mut changed = draft("Renamed");
    changed.notify = true;
    let updated = store.update_collection(two.id, changed).unwrap();
    assert_eq!((updated.name.as_str(), updated.sort_index), ("Renamed", 2));
    store.delete_collection(one.id).unwrap();
    assert_eq!(store.list_collections().unwrap().len(), 2);
}
