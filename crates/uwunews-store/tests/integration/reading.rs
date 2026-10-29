use crate::common::{source, store};
use uwunews_store::{article_key, mark_id, ArticleDraft, ArticleQuery, FetchState, StoreError};

pub fn draft(guid: &str, published_at: Option<i64>) -> ArticleDraft {
    ArticleDraft {
        guid: guid.into(),
        title: format!("Title {guid}"),
        summary: format!("about {guid}"),
        published_at,
        ..Default::default()
    }
}

#[test]
fn article_keys_and_mark_ids_are_stable() {
    let id = uuid::Uuid::parse_str("01890a5d-ac96-774b-bcce-b302099a8057").unwrap();
    let key = article_key(id, "guid-1");
    assert_eq!(key.len(), 64);
    assert_eq!(key, article_key(id, "guid-1"));
    assert_ne!(key, article_key(id, "guid-2"));
    let mark = mark_id(&key);
    assert_eq!(mark.get_version_num(), 8);
    assert_eq!(mark, mark_id(&key));
}

#[test]
fn upserting_returns_only_new_keys_and_skips_empty_guids() {
    let (store, _) = store();
    let feed = source(&store, "https://a.example.com/feed");
    let new = store
        .upsert_articles(feed.id, &[draft("1", Some(10)), draft("", Some(11))])
        .unwrap();
    assert_eq!(new, vec![article_key(feed.id, "1")]);
    let mut again = draft("1", Some(10));
    again.title = "Fixed".into();
    let new = store
        .upsert_articles(feed.id, &[again, draft("2", Some(20))])
        .unwrap();
    assert_eq!(new, vec![article_key(feed.id, "2")]);
    let all = store.articles(&ArticleQuery::default()).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].published_at, Some(20));
    assert_eq!(all[1].title, "Fixed");
    assert!(matches!(
        store.upsert_articles(uuid::Uuid::now_v7(), &[draft("x", None)]),
        Err(StoreError::UnknownSource(_))
    ));
}

#[test]
fn read_and_starred_filter_count_and_mark_all() {
    let (store, _) = store();
    let a = source(&store, "https://a.example.com/feed");
    let b = source(&store, "https://b.example.com/feed");
    store
        .upsert_articles(a.id, &[draft("1", Some(1)), draft("2", Some(2))])
        .unwrap();
    store.upsert_articles(b.id, &[draft("3", Some(3))]).unwrap();
    let one = article_key(a.id, "1");
    store.set_read(&one, true).unwrap();
    store.set_starred(&one, true).unwrap();
    assert!(matches!(
        store.set_read("nope", true),
        Err(StoreError::UnknownArticle(_))
    ));

    let counts = store.unread_counts().unwrap();
    assert_eq!((counts[&a.id], counts[&b.id]), (1, 1));
    let starred = store
        .articles(&ArticleQuery {
            starred_only: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(starred.len(), 1);
    assert!(starred[0].read && starred[0].starred);
    let found = store
        .articles(&ArticleQuery {
            search: Some("about 3".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(found.len(), 1);

    assert_eq!(store.mark_all_read(Some(vec![a.id])).unwrap(), 1);
    assert_eq!(
        store
            .unread_counts()
            .unwrap()
            .get(&a.id)
            .copied()
            .unwrap_or(0),
        0
    );
    assert_eq!(store.mark_all_read(None).unwrap(), 1);
    // One mark per article, however often it was written.
    assert_eq!(store.pending_count().unwrap(), 2 + 3);
}

#[test]
fn notifications_go_out_once_and_fetch_state_round_trips() {
    let (store, _) = store();
    let feed = source(&store, "https://a.example.com/feed");
    let keys = store
        .upsert_articles(feed.id, &[draft("1", Some(1)), draft("2", Some(2))])
        .unwrap();
    assert_eq!(store.take_unnotified(&keys).unwrap().len(), 2);
    assert!(store.take_unnotified(&keys).unwrap().is_empty());

    let state = FetchState {
        etag: Some("\"abc\"".into()),
        last_success: Some(5),
        ..Default::default()
    };
    store.set_fetch_state(feed.id, &state).unwrap();
    assert_eq!(store.fetch_state(feed.id).unwrap(), Some(state));
}

#[test]
fn pruning_keeps_starred_and_does_not_bring_old_items_back() {
    let (store, _) = store();
    let feed = source(&store, "https://a.example.com/feed");
    store
        .upsert_articles(
            feed.id,
            &[
                draft("old", Some(1)),
                draft("starred", Some(1)),
                draft("new", Some(i64::MAX / 2)),
            ],
        )
        .unwrap();
    store
        .set_starred(&article_key(feed.id, "starred"), true)
        .unwrap();
    let report = store.prune(1_000).unwrap();
    assert_eq!(report.articles, 1);
    // The starred mark is pending, so it stays either way.
    assert_eq!(report.marks, 0);
    let left: Vec<_> = store
        .articles(&ArticleQuery::default())
        .unwrap()
        .into_iter()
        .map(|a| a.guid)
        .collect();
    assert_eq!(left, vec!["new", "starred"]);
    let again = store
        .upsert_articles(feed.id, &[draft("old", Some(1))])
        .unwrap();
    assert!(again.is_empty());
}

#[test]
fn paging_does_not_skip_articles_of_the_same_time() {
    let (store, _) = store();
    let feed = source(&store, "https://a.example.com/feed");
    let drafts: Vec<_> = (0..5).map(|i| draft(&i.to_string(), Some(7))).collect();
    store.upsert_articles(feed.id, &drafts).unwrap();
    let mut seen = Vec::new();
    let mut query = ArticleQuery {
        limit: 2,
        ..Default::default()
    };
    loop {
        let page = store.articles(&query).unwrap();
        let Some(last) = page.last() else { break };
        query.before_published = Some(7);
        query.before_key = Some(last.key.clone());
        seen.extend(page.into_iter().map(|a| a.guid));
    }
    seen.sort();
    assert_eq!(seen, vec!["0", "1", "2", "3", "4"]);
}
