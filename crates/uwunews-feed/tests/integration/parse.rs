use uwunews_feed::parse;

use crate::fixture;

#[test]
fn rss2_newest_first_with_content_and_image() {
    let feed = parse(&fixture("rss2.xml"), Some("https://news.example.com/rss")).unwrap();
    assert_eq!(feed.title, "Beispiel & News");
    assert_eq!(feed.site_url.as_deref(), Some("https://news.example.com/"));
    assert_eq!(feed.items.len(), 2);
    let new = &feed.items[0];
    assert_eq!(new.guid, "neu-2");
    assert_eq!(new.title, "Neuer Artikel");
    assert_eq!(new.summary, "Teaser");
    assert!(new.content_html.contains("Volltext"));
    assert_eq!(
        new.image_url.as_deref(),
        Some("https://img.example.com/neu.jpg")
    );
    assert_eq!(new.published_at, Some(1_793_179_800_000));
    let old = &feed.items[1];
    assert_eq!(
        old.link.as_deref(),
        Some("https://news.example.com/2026/alt")
    );
    assert_eq!(old.summary, "Kurz gesagt");
    // Only a summary: the reader still gets it.
    assert!(old.content_html.contains("gesagt"));
}

#[test]
fn atom_and_json_feed() {
    let atom = parse(&fixture("atom.xml"), None).unwrap();
    assert_eq!(atom.title, "Ein Blog");
    assert_eq!(atom.site_url.as_deref(), Some("https://blog.example.org/"));
    assert_eq!(atom.items[0].author.as_deref(), Some("Nyu"));
    assert_eq!(atom.items[0].summary, "Zusammenfassung");

    let json = parse(&fixture("feed.json"), None).unwrap();
    assert_eq!(json.items[0].title, "Eins");
    assert_eq!(json.items[0].guid, "j1");
}

#[test]
fn html_is_not_a_feed() {
    assert!(parse(b"<!doctype html><html><body>Hallo</body></html>", None).is_err());
}

#[test]
fn item_keys_are_stable_and_per_source() {
    let a = uwunews_feed::item_key("s1", "g");
    assert_eq!(a, uwunews_feed::item_key("s1", "g"));
    assert_ne!(a, uwunews_feed::item_key("s2", "g"));
    assert_eq!(a.len(), 32);
}

#[test]
fn only_web_links_survive() {
    let rss = br#"<?xml version="1.0"?><rss version="2.0"><channel><title>T</title>
        <link>javascript:alert(1)</link>
        <item><guid>a</guid><title>A</title><link>shortcuts://run-shortcut?name=x</link></item>
        <item><guid>b</guid><title>B</title><link>/posts/b</link></item>
        </channel></rss>"#;
    let feed = parse(rss, Some("https://news.example.com/feed.xml")).unwrap();
    assert_eq!(feed.site_url, None);
    let links: Vec<_> = feed.items.iter().map(|i| i.link.as_deref()).collect();
    assert!(links.contains(&None));
    assert!(links.contains(&Some("https://news.example.com/posts/b")));
}
