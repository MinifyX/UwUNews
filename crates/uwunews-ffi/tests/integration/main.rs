//! The engine as the app drives it, against feeds served from this process.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use uwunews_ffi::*;

#[derive(Default)]
struct Secrets(Mutex<HashMap<String, Vec<u8>>>);

impl SecretStore for Secrets {
    fn get(&self, name: String) -> Option<Vec<u8>> {
        self.0.lock().get(&name).cloned()
    }
    fn set(&self, name: String, value: Vec<u8>) {
        self.0.lock().insert(name, value);
    }
    fn delete(&self, name: String) {
        self.0.lock().remove(&name);
    }
}

/// Serves `/a.xml`, `/b.xml`, … from a map the test changes as it goes.
struct Feeds {
    base: String,
    bodies: Arc<Mutex<HashMap<String, String>>>,
}

impl Feeds {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let bodies: Arc<Mutex<HashMap<String, String>>> = Arc::default();
        let served = bodies.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                let answer = match served.lock().get(&path) {
                    Some(body) => format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/rss+xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    ),
                    None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
                };
                let _ = stream.write_all(answer.as_bytes());
            }
        });
        Self { base, bodies }
    }

    fn serve(&self, path: &str, title: &str, items: &[&str]) -> String {
        let items: String = items
            .iter()
            .map(|t| {
                format!(
                    "<item><title>{t}</title><guid>{t}</guid><link>https://news.example/{}</link>\
                     <description>About {t}</description></item>",
                    t.replace(' ', "-")
                )
            })
            .collect();
        let body = format!(
            "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>{title}</title>\
             <link>https://news.example/</link>{items}</channel></rss>"
        );
        self.bodies.lock().insert(path.into(), body);
        format!("{}{path}", self.base)
    }
}

fn engine(dir: &std::path::Path) -> Arc<Engine> {
    Engine::new(
        dir.to_string_lossy().into(),
        "Test".into(),
        Arc::new(Secrets::default()),
    )
    .unwrap()
}

fn query(scope: Scope) -> ArticleQuery {
    ArticleQuery {
        scope,
        unread_only: false,
        search: None,
        limit: 50,
        before: None,
        before_key: None,
    }
}

fn temp() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("uwunews-ffi-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn sources_refresh_and_announce() {
    let feeds = Feeds::start();
    let dir = temp();
    let engine = engine(&dir);

    let loud = feeds.serve("/a.xml", "Loud", &["one", "two"]);
    let quiet = feeds.serve("/b.xml", "Quiet", &["old news"]);
    let preview = engine.preview(loud.clone()).unwrap();
    assert_eq!((preview.title.as_str(), preview.item_count), ("Loud", 2));

    let a = engine.add_source(loud, true, vec![]).unwrap();
    let rust = engine
        .save_collection(FeedCollection {
            id: String::new(),
            name: "Rust".into(),
            symbol: "gear".into(),
            color: "pink".into(),
            all_sources: false,
            source_ids: vec![],
            any: vec!["rust".into()],
            all: vec![],
            exclude: vec![],
            search_content: false,
            notify: true,
            unread: 0,
        })
        .unwrap();
    let b = engine
        .add_source(quiet, false, vec![rust.id.clone()])
        .unwrap();
    assert_eq!(engine.counts().unwrap().unread, 3);
    assert_eq!(engine.collections().unwrap()[0].unread, 0);

    // Nothing new yet: nothing to say.
    let report = engine.refresh(None);
    assert_eq!(
        (report.checked, report.new_articles, report.failed),
        (2, 0, 0)
    );
    assert!(report.notices.is_empty());

    feeds.serve("/a.xml", "Loud", &["three", "one", "two"]);
    feeds.serve(
        "/b.xml",
        "Quiet",
        &["Rust 2.0 is out", "cooking", "old news"],
    );
    let report = engine.refresh(None);
    assert_eq!(report.new_articles, 3);
    let mut said: Vec<_> = report
        .notices
        .iter()
        .map(|n| (n.title.as_str(), n.collection_name.as_deref()))
        .collect();
    said.sort();
    assert_eq!(said, [("Rust 2.0 is out", Some("Rust")), ("three", None)]);
    // Announced once only.
    assert!(engine.refresh(None).notices.is_empty());

    let in_rust = engine
        .articles(query(Scope::Collection {
            id: rust.id.clone(),
        }))
        .unwrap();
    assert_eq!(in_rust.len(), 1);
    assert_eq!(in_rust[0].source_title, "Quiet");
    assert_eq!(engine.collections().unwrap()[0].unread, 1);

    engine
        .mark_all_read(Scope::Collection { id: rust.id })
        .unwrap();
    assert_eq!(engine.counts().unwrap().unread, 5);
    engine
        .mark_all_read(Scope::Source { id: a.id.clone() })
        .unwrap();
    let left = engine
        .articles(ArticleQuery {
            unread_only: true,
            ..query(Scope::All)
        })
        .unwrap();
    assert_eq!(left.len(), 2);
    assert!(left.iter().all(|x| x.source_id == b.id));

    engine.set_starred(left[0].key.clone(), true).unwrap();
    assert_eq!(engine.counts().unwrap().starred, 1);
    let detail = engine.article(left[0].key.clone()).unwrap().unwrap();
    assert!(detail.article.starred);
    assert_eq!(detail.site_url.as_deref(), Some("https://news.example/"));

    // A broken source fails on its own and says so.
    feeds.bodies.lock().remove("/a.xml");
    let report = engine.refresh(None);
    assert_eq!(report.failed, 1);
    let sources = engine.sources().unwrap();
    let loud = sources.iter().find(|s| s.id == a.id).unwrap();
    assert!(loud.last_error.is_some());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn opml_round_trip() {
    let dir = temp();
    let engine = engine(&dir);
    let opml = br#"<?xml version="1.0"?><opml version="2.0"><body>
        <outline text="Alone" xmlUrl="https://one.example/feed"/>
        <outline text="Tech">
          <outline text="Two" xmlUrl="https://two.example/rss" htmlUrl="https://two.example/"/>
          <outline text="Three" xmlUrl="https://three.example/atom"/>
        </outline></body></opml>"#;
    assert_eq!(engine.import_opml(opml.to_vec()).unwrap(), 3);
    // The same file again adds nothing.
    assert_eq!(engine.import_opml(opml.to_vec()).unwrap(), 0);
    let collections = engine.collections().unwrap();
    assert_eq!(collections.len(), 1);
    assert_eq!(
        (
            collections[0].name.as_str(),
            collections[0].source_ids.len()
        ),
        ("Tech", 2)
    );

    let out = engine.export_opml().unwrap();
    let other = temp();
    let again = self::engine(&other);
    assert_eq!(again.import_opml(out.into_bytes()).unwrap(), 3);
    assert_eq!(again.collections().unwrap()[0].source_ids.len(), 2);
    assert_eq!(again.sync_status().unwrap().backend, SyncBackend::Off);
    std::fs::remove_dir_all(dir).ok();
    std::fs::remove_dir_all(other).ok();
}
