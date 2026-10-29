use uwunews_feed::discover::{links_in_page, normalise};
use uwunews_feed::fetch::{Answer, Validators};

#[test]
fn normalises_what_people_paste() {
    assert_eq!(
        normalise(" news.example.com ").unwrap(),
        "https://news.example.com/"
    );
    assert_eq!(
        normalise("feed://news.example.com/rss").unwrap(),
        "https://news.example.com/rss"
    );
    assert!(normalise("ftp://news.example.com").is_err());
    assert!(normalise("").is_err());
}

#[test]
fn finds_alternate_links() {
    let answer = Answer {
        url: "https://site.example.com/blog/".into(),
        content_type: "text/html".into(),
        body: br#"<html><head>
            <link rel="stylesheet" href="/a.css">
            <link rel="alternate" type="application/rss+xml" title="RSS" href="/feed.xml">
            <LINK REL='alternate' TYPE='application/atom+xml' HREF='https://cdn.example.com/atom'>
            <link rel="alternate" hreflang="en" href="/en/">
        </head></html>"#
            .to_vec(),
        validators: Validators::default(),
    };
    assert_eq!(
        links_in_page(&answer),
        vec![
            "https://site.example.com/feed.xml",
            "https://cdn.example.com/atom"
        ]
    );
}
