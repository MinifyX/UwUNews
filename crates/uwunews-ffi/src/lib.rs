//! The engine the UwUNews apps talk to: the store, the fetcher and the sync
//! client behind one object, in the shapes the screens use.
//!
//! Every call blocks (SQLite, HTTP); the app calls from a background task.
//! Times cross as unix milliseconds.

use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;
use uwunews_feed::{discover, opml, Fetched, Fetcher, Rules, SearchIn, Validators};
use uwunews_store::{
    ArticleDraft, ArticleQuery as StoreQuery, ArticleRecord, CollectionDraft, CollectionRecord,
    CollectionRules, FetchState, SourceDraft, SourceRecord, Store, StoreError,
};
use uwunews_sync::client::SyncClient;

mod sync;

pub use sync::*;

uniffi::setup_scaffolding!();

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// Feeds fetched at once.
const WORKERS: usize = 6;
const SEARCH_TITLE: &str = "title";
const SEARCH_CONTENT: &str = "title_and_content";

// ── What crosses ────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum NewsError {
    #[error("{message}")]
    Network { message: String },
    #[error("{message}")]
    NotAFeed { message: String },
    #[error("{message}")]
    Invalid { message: String },
    #[error("{message}")]
    Storage { message: String },
    #[error("{message}")]
    Sync { message: String },
}

type Result<T, E = NewsError> = std::result::Result<T, E>;

fn invalid(message: impl Into<String>) -> NewsError {
    NewsError::Invalid {
        message: message.into(),
    }
}

impl From<StoreError> for NewsError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Invalid { .. } | StoreError::DuplicateSource => invalid(error.to_string()),
            StoreError::UnknownSource(_)
            | StoreError::UnknownCollection(_)
            | StoreError::UnknownArticle(_) => invalid(error.to_string()),
            other => NewsError::Storage {
                message: other.to_string(),
            },
        }
    }
}

impl From<uwunews_feed::Error> for NewsError {
    fn from(error: uwunews_feed::Error) -> Self {
        use uwunews_feed::Error as E;
        let message = error.to_string();
        match error {
            E::Network(_) | E::Status(_) | E::TooLarge(_) => NewsError::Network { message },
            E::NotAFeed(_) => NewsError::NotAFeed { message },
            E::Url(_) | E::Opml(_) => NewsError::Invalid { message },
        }
    }
}

/// Where a key lives that must not sit in the database: the Keychain on iOS.
#[uniffi::export(with_foreign)]
pub trait SecretStore: Send + Sync {
    fn get(&self, name: String) -> Option<Vec<u8>>;
    fn set(&self, name: String, value: Vec<u8>);
    fn delete(&self, name: String);
}

struct Secrets(Arc<dyn SecretStore>);

impl uwunews_store::SecretStore for Secrets {
    fn get(&self, name: &str) -> Option<Vec<u8>> {
        self.0.get(name.to_string())
    }
    fn set(&self, name: &str, value: &[u8]) {
        self.0.set(name.to_string(), value.to_vec())
    }
    fn delete(&self, name: &str) {
        self.0.delete(name.to_string())
    }
}

/// Which articles a list shows.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum Scope {
    All,
    /// The last 24 hours.
    Today,
    Starred,
    Source {
        id: String,
    },
    Collection {
        id: String,
    },
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ArticleQuery {
    pub scope: Scope,
    pub unread_only: bool,
    pub search: Option<String>,
    pub limit: u32,
    /// Only after this article in the order (older, or as old with a later
    /// key), to page on: its `sort_at` and key.
    pub before: Option<i64>,
    pub before_key: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct Article {
    pub key: String,
    pub source_id: String,
    pub source_title: String,
    pub title: String,
    pub summary: String,
    pub link: Option<String>,
    pub author: Option<String>,
    pub image_url: Option<String>,
    pub published_at: Option<i64>,
    /// What lists sort by: the publication time, or when it was fetched.
    pub sort_at: i64,
    pub read: bool,
    pub starred: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ArticleDetail {
    pub article: Article,
    pub content_html: String,
    pub site_url: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FeedSource {
    pub id: String,
    pub title: String,
    pub url: String,
    pub site_url: Option<String>,
    pub notify: bool,
    pub unread: u32,
    pub last_fetched_at: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FeedCollection {
    /// Empty for one not saved yet.
    pub id: String,
    pub name: String,
    pub symbol: String,
    pub color: String,
    pub all_sources: bool,
    pub source_ids: Vec<String>,
    pub any: Vec<String>,
    pub all: Vec<String>,
    pub exclude: Vec<String>,
    pub search_content: bool,
    pub notify: bool,
    pub unread: u32,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct Counts {
    pub unread: u32,
    pub starred: u32,
    pub today: u32,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FeedPreview {
    pub url: String,
    pub title: String,
    pub site_url: Option<String>,
    pub item_count: u32,
    /// The newest few titles.
    pub latest: Vec<String>,
}

/// One new article worth a notification.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Notice {
    pub key: String,
    pub title: String,
    pub source_id: String,
    pub source_title: String,
    /// The collection that asked for it, when the source itself is quiet.
    pub collection_name: Option<String>,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct RefreshReport {
    pub checked: u32,
    pub new_articles: u32,
    pub failed: u32,
    pub errors: Vec<String>,
    pub notices: Vec<Notice>,
}

// ── The engine ──────────────────────────────────────────────────────────────

#[derive(uniffi::Object)]
pub struct Engine {
    store: Arc<Store>,
    fetcher: Fetcher,
    sync: SyncClient,
    cancelled: AtomicBool,
    pairing: Mutex<Option<uwunews_sync::flow::PairingOffer>>,
    /// The id of the pairing being offered, until it is done or cancelled.
    open_pairing: Mutex<Option<String>>,
    /// One sync pass at a time, whoever asks: the app or the background task.
    syncing: Mutex<()>,
}

#[uniffi::export]
impl Engine {
    /// Opens (or makes) the database in `data_dir`. `device_name` is what
    /// other devices list this one as.
    #[uniffi::constructor]
    pub fn new(
        data_dir: String,
        device_name: String,
        secrets: Arc<dyn SecretStore>,
    ) -> Result<Arc<Self>> {
        let dir = Path::new(&data_dir);
        std::fs::create_dir_all(dir).map_err(|e| NewsError::Storage {
            message: e.to_string(),
        })?;
        let store = Arc::new(Store::open(
            &dir.join("uwunews.sqlite"),
            Arc::new(Secrets(secrets)),
        )?);
        let fetcher = Fetcher::new()?;
        Ok(Arc::new(Self {
            sync: SyncClient::new(store.clone(), device_name),
            store,
            fetcher,
            cancelled: AtomicBool::new(false),
            pairing: Mutex::new(None),
            open_pairing: Mutex::new(None),
            syncing: Mutex::new(()),
        }))
    }

    // ── Reading ─────────────────────────────────────────────────────────────

    pub fn sources(&self) -> Result<Vec<FeedSource>> {
        let unread = self.store.unread_counts()?;
        self.store
            .list_sources()?
            .into_iter()
            .map(|record| {
                let state = self.store.fetch_state(record.id)?.unwrap_or_default();
                Ok(source_out(&record, &unread, &state))
            })
            .collect()
    }

    pub fn collections(&self) -> Result<Vec<FeedCollection>> {
        let unread = self.store.unread_counts()?;
        let sources = self.source_ids()?;
        self.store
            .list_collections()?
            .into_iter()
            .map(|record| {
                let count = self.collection_unread(&record, &sources, &unread)?;
                Ok(collection_out(&record, count))
            })
            .collect()
    }

    pub fn counts(&self) -> Result<Counts> {
        let unread = self.store.unread_counts()?.values().sum::<usize>();
        let starred = self.store.count_articles(&StoreQuery {
            starred_only: true,
            ..StoreQuery::default()
        })?;
        let today = self.store.count_articles(&StoreQuery {
            unread_only: true,
            since: Some(now_ms() - DAY_MS),
            ..StoreQuery::default()
        })?;
        Ok(Counts {
            unread: unread as u32,
            starred: starred as u32,
            today: today as u32,
        })
    }

    pub fn articles(&self, query: ArticleQuery) -> Result<Vec<Article>> {
        let titles = self.titles()?;
        let limit = query.limit as usize;
        let mut base = StoreQuery {
            unread_only: query.unread_only,
            search: query.search.filter(|s| !s.trim().is_empty()),
            before_published: query.before,
            before_key: query.before_key,
            limit,
            ..StoreQuery::default()
        };
        let rules = match &query.scope {
            Scope::All => None,
            Scope::Today => {
                base.since = Some(now_ms() - DAY_MS);
                None
            }
            Scope::Starred => {
                base.starred_only = true;
                None
            }
            Scope::Source { id } => {
                base.source_ids = Some(vec![parse_id(id)?]);
                None
            }
            Scope::Collection { id } => {
                let collection = self.collection_record(id)?;
                base.source_ids = Some(self.members(&collection)?);
                Some(rules_of(&collection.rules)).filter(|r| !r.is_empty())
            }
        };
        let records = match rules {
            None => self.store.articles(&base)?,
            Some(rules) => self.filtered(base, &rules)?,
        };
        Ok(records.iter().map(|r| article_out(r, &titles)).collect())
    }

    pub fn article(&self, key: String) -> Result<Option<ArticleDetail>> {
        let Some(record) = self.store.article(&key)? else {
            return Ok(None);
        };
        let source = self.store.source(record.source_id)?;
        let titles = self.titles()?;
        Ok(Some(ArticleDetail {
            article: article_out(&record, &titles),
            site_url: source.and_then(|s| s.site_url),
            content_html: record.content_html,
        }))
    }

    // ── Marks ───────────────────────────────────────────────────────────────

    pub fn set_read(&self, keys: Vec<String>, read: bool) -> Result<()> {
        for key in keys {
            self.store.set_read(&key, read)?;
        }
        Ok(())
    }

    pub fn set_starred(&self, key: String, starred: bool) -> Result<()> {
        self.store.set_starred(&key, starred)?;
        Ok(())
    }

    pub fn mark_all_read(&self, scope: Scope) -> Result<()> {
        match &scope {
            Scope::All => {
                self.store.mark_all_read(None)?;
            }
            Scope::Source { id } => {
                self.store.mark_all_read(Some(vec![parse_id(id)?]))?;
            }
            Scope::Collection { id } => {
                let collection = self.collection_record(id)?;
                if rules_of(&collection.rules).is_empty() {
                    self.store.mark_all_read(Some(self.members(&collection)?))?;
                    return Ok(());
                }
                self.mark_listed(scope)?;
            }
            Scope::Today | Scope::Starred => self.mark_listed(scope)?,
        }
        Ok(())
    }

    // ── Sources ─────────────────────────────────────────────────────────────

    /// Finds the feed behind an address (a feed, or a page that links one)
    /// and shows what it holds, without adding it.
    pub fn preview(&self, input: String) -> Result<FeedPreview> {
        let found = discover::discover(&self.fetcher, &input)?;
        Ok(FeedPreview {
            title: found.feed.title.clone(),
            site_url: found.feed.site_url.clone(),
            item_count: found.feed.items.len() as u32,
            latest: found
                .feed
                .items
                .iter()
                .take(3)
                .map(|i| i.title.clone())
                .collect(),
            url: found.url,
        })
    }

    /// Adds the feed behind `input`, stores what it lists now (without
    /// announcing any of it) and puts it into the given collections.
    pub fn add_source(
        &self,
        input: String,
        notify: bool,
        collection_ids: Vec<String>,
    ) -> Result<FeedSource> {
        let found = discover::discover(&self.fetcher, &input)?;
        let record = self.store.add_source(SourceDraft {
            url: found.url.clone(),
            title: Some(found.feed.title.clone()).filter(|t| !t.trim().is_empty()),
            site_url: found.feed.site_url.clone(),
            notify,
        })?;
        let fresh = self
            .store
            .upsert_articles(record.id, &drafts(&found.feed))?;
        self.store.take_unnotified(&fresh)?;
        let now = now_ms();
        let state = FetchState {
            etag: found.validators.etag,
            last_modified: found.validators.last_modified,
            last_attempt: Some(now),
            last_success: Some(now),
            last_error: None,
        };
        self.store.set_fetch_state(record.id, &state)?;
        for id in collection_ids {
            let collection = self.collection_record(&id)?;
            if collection.all_sources || collection.source_ids.contains(&record.id) {
                continue;
            }
            let mut draft = draft_of(&collection);
            draft.source_ids.push(record.id);
            self.store.update_collection(collection.id, draft)?;
        }
        let unread = self.store.unread_counts()?;
        Ok(source_out(&record, &unread, &state))
    }

    /// Renames a source (empty: back to the feed's own title) and switches
    /// its notifications.
    pub fn update_source(&self, id: String, title: String, notify: bool) -> Result<()> {
        let title = Some(title.trim().to_string()).filter(|t| !t.is_empty());
        self.store.update_source(parse_id(&id)?, title, notify)?;
        Ok(())
    }

    pub fn delete_source(&self, id: String) -> Result<()> {
        self.store.delete_source(parse_id(&id)?)?;
        Ok(())
    }

    // ── Collections ─────────────────────────────────────────────────────────

    /// Creates the collection when its id is empty, otherwise changes it.
    pub fn save_collection(&self, collection: FeedCollection) -> Result<FeedCollection> {
        let ids = collection
            .source_ids
            .iter()
            .map(|id| parse_id(id))
            .collect::<Result<Vec<_>>>()?;
        let draft = CollectionDraft {
            name: collection.name.trim().to_string(),
            symbol: collection.symbol,
            color: collection.color,
            all_sources: collection.all_sources,
            source_ids: ids,
            rules: CollectionRules {
                any_keywords: words(collection.any),
                all_keywords: words(collection.all),
                exclude_keywords: words(collection.exclude),
                search_in: if collection.search_content {
                    SEARCH_CONTENT
                } else {
                    SEARCH_TITLE
                }
                .to_string(),
            },
            notify: collection.notify,
        };
        let record = if collection.id.is_empty() {
            self.store.add_collection(draft)?
        } else {
            self.store
                .update_collection(parse_id(&collection.id)?, draft)?
        };
        let unread = self.store.unread_counts()?;
        let count = self.collection_unread(&record, &self.source_ids()?, &unread)?;
        Ok(collection_out(&record, count))
    }

    pub fn delete_collection(&self, id: String) -> Result<()> {
        self.store.delete_collection(parse_id(&id)?)?;
        Ok(())
    }

    pub fn order_collections(&self, ids: Vec<String>) -> Result<()> {
        let ids = ids
            .iter()
            .map(|id| parse_id(id))
            .collect::<Result<Vec<_>>>()?;
        self.store.reorder_collections(&ids)?;
        Ok(())
    }

    // ── Fetching ────────────────────────────────────────────────────────────

    /// Fetches every source, or the one given, and says what is new and
    /// which of it to announce. Failures land in the report and on the
    /// source, never as an error.
    pub fn refresh(&self, source_id: Option<String>) -> RefreshReport {
        let mut report = RefreshReport::default();
        let sources = match self.refresh_targets(source_id.as_deref()) {
            Ok(sources) => sources,
            Err(error) => {
                report.failed = 1;
                report.errors.push(error.to_string());
                return report;
            }
        };
        let collections = self.store.list_collections().unwrap_or_default();
        let report = Mutex::new(report);
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..WORKERS.min(sources.len()) {
                scope.spawn(|| loop {
                    if self.cancelled.load(Ordering::SeqCst) {
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(source) = sources.get(i) else {
                        return;
                    };
                    let outcome = self.refresh_one(source, &collections);
                    let mut report = report.lock();
                    report.checked += 1;
                    match outcome {
                        Ok((fresh, notices)) => {
                            report.new_articles += fresh;
                            report.notices.extend(notices);
                        }
                        Err(error) => {
                            report.failed += 1;
                            report.errors.push(format!("{}: {error}", title_of(source)));
                        }
                    }
                });
            }
        });
        // Cleared only now: a cancel that came before the workers started
        // still counts.
        self.cancelled.store(false, Ordering::SeqCst);
        report.into_inner()
    }

    /// Stops a refresh after the feeds already on their way.
    /// Also stops the next one if none is running yet.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Forgets read, unstarred articles older than `keep_days`; says how many.
    pub fn prune(&self, keep_days: u32) -> Result<u32> {
        let report = self
            .store
            .prune(now_ms() - i64::from(keep_days.max(1)) * DAY_MS)?;
        Ok(report.articles as u32)
    }

    // ── OPML ────────────────────────────────────────────────────────────────

    /// Adds every feed of an OPML file not followed yet, folders as
    /// collections. Says how many sources came in; nothing is fetched.
    pub fn import_opml(&self, data: Vec<u8>) -> Result<u32> {
        if data.len() > MAX_OPML {
            return Err(invalid("the file is too large for a list of feeds"));
        }
        let file = opml::read(&data)?;
        let mut by_url: HashMap<String, Uuid> = self
            .store
            .list_sources()?
            .into_iter()
            .map(|s| (s.url, s.id))
            .collect();
        let mut added = 0;
        let mut add = |outline: &opml::Outline| -> Result<Option<Uuid>> {
            let Ok(url) = uwunews_feed::discover::normalise(&outline.xml_url) else {
                return Ok(None);
            };
            if let Some(id) = by_url.get(&url) {
                return Ok(Some(*id));
            }
            let draft = SourceDraft {
                url,
                title: Some(outline.title.clone()).filter(|t| !t.trim().is_empty()),
                site_url: outline.html_url.clone(),
                notify: false,
            };
            match self.store.add_source(draft) {
                Ok(record) => {
                    added += 1;
                    by_url.insert(record.url.clone(), record.id);
                    Ok(Some(record.id))
                }
                Err(StoreError::DuplicateSource | StoreError::Invalid { .. }) => Ok(None),
                Err(error) => Err(error.into()),
            }
        };
        for outline in &file.feeds {
            add(outline)?;
        }
        let mut existing = self.store.list_collections()?;
        for (name, outlines) in &file.folders {
            let mut ids = Vec::new();
            for outline in outlines {
                if let Some(id) = add(outline)? {
                    ids.push(id);
                }
            }
            if ids.is_empty() {
                continue;
            }
            match existing.iter().find(|c| c.name == *name) {
                Some(collection) if !collection.all_sources => {
                    let mut draft = draft_of(collection);
                    for id in ids {
                        if !draft.source_ids.contains(&id) {
                            draft.source_ids.push(id);
                        }
                    }
                    self.store.update_collection(collection.id, draft)?;
                }
                Some(_) => {}
                None => {
                    let made = self.store.add_collection(CollectionDraft {
                        name: name.clone(),
                        symbol: "folder".into(),
                        color: "pink".into(),
                        all_sources: false,
                        source_ids: ids,
                        rules: CollectionRules {
                            search_in: SEARCH_TITLE.into(),
                            ..CollectionRules::default()
                        },
                        notify: false,
                    })?;
                    // A second folder of the same name joins this one.
                    existing.push(made);
                }
            }
        }
        Ok(added)
    }

    /// Every source, those in a hand-picked collection inside its folder.
    pub fn export_opml(&self) -> Result<String> {
        let sources = self.store.list_sources()?;
        let outline = |s: &SourceRecord| opml::Outline {
            title: title_of(s),
            xml_url: s.url.clone(),
            html_url: s.site_url.clone(),
        };
        let mut filed = HashSet::new();
        let mut folders = Vec::new();
        for collection in self.store.list_collections()? {
            if collection.all_sources || !rules_of(&collection.rules).is_empty() {
                continue;
            }
            let inside: Vec<_> = sources
                .iter()
                .filter(|s| collection.source_ids.contains(&s.id))
                .collect();
            if inside.is_empty() {
                continue;
            }
            filed.extend(inside.iter().map(|s| s.id));
            folders.push((collection.name, inside.into_iter().map(outline).collect()));
        }
        let feeds = sources
            .iter()
            .filter(|s| !filed.contains(&s.id))
            .map(outline)
            .collect();
        Ok(opml::write(&opml::Opml { feeds, folders }))
    }
}

impl Engine {
    fn titles(&self) -> Result<HashMap<Uuid, String>> {
        Ok(self
            .store
            .list_sources()?
            .iter()
            .map(|s| (s.id, title_of(s)))
            .collect())
    }

    fn source_ids(&self) -> Result<Vec<Uuid>> {
        Ok(self.store.list_sources()?.iter().map(|s| s.id).collect())
    }

    fn collection_record(&self, id: &str) -> Result<CollectionRecord> {
        let id = parse_id(id)?;
        self.store
            .list_collections()?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| StoreError::UnknownCollection(id).into())
    }

    /// The sources a collection draws from.
    fn members(&self, collection: &CollectionRecord) -> Result<Vec<Uuid>> {
        if collection.all_sources {
            self.source_ids()
        } else {
            Ok(collection.source_ids.clone())
        }
    }

    fn collection_unread(
        &self,
        collection: &CollectionRecord,
        all: &[Uuid],
        unread: &HashMap<Uuid, usize>,
    ) -> Result<u32> {
        let members = if collection.all_sources {
            all
        } else {
            &collection.source_ids
        };
        let rules = rules_of(&collection.rules);
        if rules.is_empty() {
            let n: usize = members.iter().filter_map(|id| unread.get(id)).sum();
            return Ok(n as u32);
        }
        let records = self.store.articles(&StoreQuery {
            source_ids: Some(members.to_vec()),
            unread_only: true,
            ..StoreQuery::default()
        })?;
        Ok(records.iter().filter(|r| passes(&rules, r)).count() as u32)
    }

    /// A page of what a filter feed lets through: pages of its sources'
    /// articles until enough pass.
    fn filtered(&self, mut query: StoreQuery, rules: &Rules) -> Result<Vec<ArticleRecord>> {
        let want = query.limit;
        query.limit = if want == 0 { 0 } else { (want * 3).max(100) };
        let mut out = Vec::new();
        loop {
            let batch = self.store.articles(&query)?;
            let full = query.limit > 0 && batch.len() == query.limit;
            let last = batch.last().map(|r| (sort_at(r), r.key.clone()));
            out.extend(batch.into_iter().filter(|r| passes(rules, r)));
            if want > 0 && out.len() >= want {
                out.truncate(want);
                break;
            }
            match last {
                Some((at, key)) if full => {
                    query.before_published = Some(at);
                    query.before_key = Some(key);
                }
                _ => break,
            }
        }
        Ok(out)
    }

    fn mark_listed(&self, scope: Scope) -> Result<()> {
        let keys: Vec<String> = self
            .articles(ArticleQuery {
                scope,
                unread_only: true,
                search: None,
                limit: 0,
                before: None,
                before_key: None,
            })?
            .into_iter()
            .map(|a| a.key)
            .collect();
        self.set_read(keys, true)
    }

    fn refresh_targets(&self, only: Option<&str>) -> Result<Vec<SourceRecord>> {
        match only {
            None => Ok(self.store.list_sources()?),
            Some(id) => {
                let id = parse_id(id)?;
                Ok(vec![self
                    .store
                    .source(id)?
                    .ok_or(StoreError::UnknownSource(id))?])
            }
        }
    }

    /// Fetches one source; the number of new articles and what to announce.
    fn refresh_one(
        &self,
        source: &SourceRecord,
        collections: &[CollectionRecord],
    ) -> Result<(u32, Vec<Notice>)> {
        let mut state = self.store.fetch_state(source.id)?.unwrap_or_default();
        let first = state.last_success.is_none();
        let validators = Validators {
            etag: state.etag.clone(),
            last_modified: state.last_modified.clone(),
        };
        state.last_attempt = Some(now_ms());
        let fetched = match self.fetcher.fetch(&source.url, &validators) {
            Ok(fetched) => fetched,
            Err(error) => {
                state.last_error = Some(error.to_string());
                self.store.set_fetch_state(source.id, &state)?;
                return Err(error.into());
            }
        };
        state.last_success = state.last_attempt;
        state.last_error = None;
        let Fetched::Feed {
            feed, validators, ..
        } = fetched
        else {
            self.store.set_fetch_state(source.id, &state)?;
            return Ok((0, Vec::new()));
        };
        state.etag = validators.etag;
        state.last_modified = validators.last_modified;
        if source.site_url.is_none() && feed.site_url.is_some() {
            self.store
                .set_source_site_url(source.id, feed.site_url.clone())?;
        }
        let fresh = self.store.upsert_articles(source.id, &drafts(&feed))?;
        self.store.set_fetch_state(source.id, &state)?;
        let unannounced = self.store.take_unnotified(&fresh)?;
        if first {
            return Ok((fresh.len() as u32, Vec::new()));
        }
        let mut notices = Vec::new();
        for key in unannounced {
            let Some(item) = feed
                .items
                .iter()
                .find(|i| uwunews_store::article_key(source.id, &i.guid) == key)
            else {
                continue;
            };
            let asked_by = if source.notify {
                Some(None)
            } else {
                collections
                    .iter()
                    .filter(|c| c.notify && (c.all_sources || c.source_ids.contains(&source.id)))
                    .find(|c| {
                        rules_of(&c.rules).matches(
                            &item.title,
                            &format!("{} {}", item.summary, item.content_html),
                        )
                    })
                    .map(|c| Some(c.name.clone()))
            };
            if let Some(collection_name) = asked_by {
                notices.push(Notice {
                    key,
                    title: item.title.clone(),
                    source_id: source.id.to_string(),
                    source_title: title_of(source),
                    collection_name,
                });
            }
        }
        Ok((fresh.len() as u32, notices))
    }
}

// ── Shapes ──────────────────────────────────────────────────────────────────

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn parse_id(id: &str) -> Result<Uuid> {
    Uuid::parse_str(id).map_err(|_| invalid(format!("not an id: {id}")))
}

fn title_of(source: &SourceRecord) -> String {
    source
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| host_of(&source.url))
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

/// OPML files are lists of addresses: even thousands of feeds stay far below.
const MAX_OPML: usize = 5 * 1024 * 1024;

fn sort_at(record: &ArticleRecord) -> i64 {
    record.published_at.unwrap_or(record.fetched_at)
}

fn words(list: Vec<String>) -> Vec<String> {
    list.into_iter()
        .map(|w| w.trim().to_string())
        .filter(|w| !w.is_empty())
        .collect()
}

fn rules_of(rules: &CollectionRules) -> Rules {
    Rules {
        any: rules.any_keywords.clone(),
        all: rules.all_keywords.clone(),
        exclude: rules.exclude_keywords.clone(),
        search_in: if rules.search_in == SEARCH_CONTENT {
            SearchIn::TitleAndContent
        } else {
            SearchIn::Title
        },
    }
}

fn passes(rules: &Rules, record: &ArticleRecord) -> bool {
    rules.matches(
        &record.title,
        &format!("{} {}", record.summary, record.content_html),
    )
}

fn draft_of(collection: &CollectionRecord) -> CollectionDraft {
    CollectionDraft {
        name: collection.name.clone(),
        symbol: collection.symbol.clone(),
        color: collection.color.clone(),
        all_sources: collection.all_sources,
        source_ids: collection.source_ids.clone(),
        rules: collection.rules.clone(),
        notify: collection.notify,
    }
}

fn drafts(feed: &uwunews_feed::Feed) -> Vec<ArticleDraft> {
    feed.items
        .iter()
        .map(|item| ArticleDraft {
            guid: item.guid.clone(),
            title: item.title.clone(),
            link: item.link.clone(),
            summary: item.summary.clone(),
            content_html: item.content_html.clone(),
            author: item.author.clone(),
            image_url: item.image_url.clone(),
            published_at: item.published_at,
        })
        .collect()
}

fn source_out(
    record: &SourceRecord,
    unread: &HashMap<Uuid, usize>,
    state: &FetchState,
) -> FeedSource {
    FeedSource {
        id: record.id.to_string(),
        title: title_of(record),
        url: record.url.clone(),
        site_url: record.site_url.clone(),
        notify: record.notify,
        unread: unread.get(&record.id).copied().unwrap_or(0) as u32,
        last_fetched_at: state.last_success,
        last_error: state.last_error.clone(),
    }
}

fn collection_out(record: &CollectionRecord, unread: u32) -> FeedCollection {
    FeedCollection {
        id: record.id.to_string(),
        name: record.name.clone(),
        symbol: record.symbol.clone(),
        color: record.color.clone(),
        all_sources: record.all_sources,
        source_ids: record.source_ids.iter().map(Uuid::to_string).collect(),
        any: record.rules.any_keywords.clone(),
        all: record.rules.all_keywords.clone(),
        exclude: record.rules.exclude_keywords.clone(),
        search_content: record.rules.search_in == SEARCH_CONTENT,
        notify: record.notify,
        unread,
    }
}

fn article_out(record: &ArticleRecord, titles: &HashMap<Uuid, String>) -> Article {
    Article {
        key: record.key.clone(),
        source_id: record.source_id.to_string(),
        source_title: titles.get(&record.source_id).cloned().unwrap_or_default(),
        title: record.title.clone(),
        summary: record.summary.clone(),
        link: record.link.clone(),
        author: record.author.clone(),
        image_url: record.image_url.clone(),
        published_at: record.published_at,
        sort_at: sort_at(record),
        read: record.read,
        starred: record.starred,
    }
}
