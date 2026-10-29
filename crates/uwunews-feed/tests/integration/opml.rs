use uwunews_feed::opml;

use crate::fixture;

#[test]
fn reads_folders_flattened() {
    let o = opml::read(&fixture("subs.opml")).unwrap();
    assert_eq!(o.feeds.len(), 1);
    assert_eq!(
        o.feeds[0].html_url.as_deref(),
        Some("https://a.example.com/")
    );
    assert_eq!(o.folders.len(), 1, "empty folders are dropped");
    let (name, feeds) = &o.folders[0];
    assert_eq!(name, "Tech");
    assert_eq!(feeds.len(), 2);
    assert_eq!(feeds[1].title, "C & D");
    assert_eq!(o.all_feeds().len(), 3);
}

#[test]
fn round_trips() {
    let o = opml::read(&fixture("subs.opml")).unwrap();
    let again = opml::read(opml::write(&o).as_bytes()).unwrap();
    assert_eq!(o, again);
}

#[test]
fn rejects_other_xml() {
    assert!(opml::read(b"<rss></rss>").is_err());
}
