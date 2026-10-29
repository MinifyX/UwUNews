//! From whatever someone pasted to a feed.
//!
//! People paste a site's address far more often than its feed's. So adding a
//! source tries, in order: the address as a feed; the `<link rel="alternate">`
//! feeds the page names; and the usual paths (`/feed`, `/rss.xml`, …) on the
//! same site.

use crate::fetch::{Answer, Fetcher, Validators};
use crate::{parse, text, Error, Feed, Result};

/// A feed found for an address.
#[derive(Debug)]
pub struct Found {
    pub url: String,
    pub feed: Feed,
    pub validators: Validators,
}

const GUESSES: [&str; 7] = [
    "/feed",
    "/rss",
    "/feed.xml",
    "/rss.xml",
    "/atom.xml",
    "/index.xml",
    "/feed.json",
];

/// Finds the feed behind `input`: a feed URL, a page URL, or a bare host
/// ("news.example" becomes "https://news.example").
pub fn discover(fetcher: &Fetcher, input: &str) -> Result<Found> {
    let url = normalise(input)?;
    let answer = fetcher
        .get(&url, &Validators::default())?
        .ok_or_else(|| Error::NotAFeed(url.clone()))?;
    if let Ok(feed) = parse(&answer.body, Some(&answer.url)) {
        return Ok(Found {
            url: answer.url,
            feed,
            validators: answer.validators,
        });
    }
    let mut candidates = links_in_page(&answer);
    let base = url::Url::parse(&answer.url).map_err(|e| Error::Url(e.to_string()))?;
    candidates.extend(
        GUESSES
            .iter()
            .filter_map(|p| base.join(p).ok())
            .map(|u| u.to_string()),
    );
    candidates.dedup();
    for candidate in candidates {
        let Ok(Some(answer)) = fetcher.get(&candidate, &Validators::default()) else {
            continue;
        };
        if let Ok(feed) = parse(&answer.body, Some(&answer.url)) {
            return Ok(Found {
                url: answer.url,
                feed,
                validators: answer.validators,
            });
        }
    }
    Err(Error::NotAFeed(url))
}

/// `https://` in front of a bare host, whitespace trimmed, `feed://` taken as
/// `https://`.
pub fn normalise(input: &str) -> Result<String> {
    let input = input.trim();
    if input.is_empty() {
        return Err(Error::Url("empty".into()));
    }
    let input = input
        .strip_prefix("feed://")
        .map(|rest| format!("https://{rest}"))
        .unwrap_or_else(|| input.to_string());
    let with_scheme = if input.contains("://") {
        input
    } else {
        format!("https://{input}")
    };
    let url = url::Url::parse(&with_scheme).map_err(|e| Error::Url(e.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(Error::Url(with_scheme));
    }
    Ok(url.to_string())
}

/// Feeds an HTML page announces in its `<head>`, resolved against the page.
pub fn links_in_page(answer: &Answer) -> Vec<String> {
    let html = String::from_utf8_lossy(&answer.body);
    let base = url::Url::parse(&answer.url).ok();
    let lower = html.to_ascii_lowercase();
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find("<link") {
        let start = from + at;
        let end = lower[start..].find('>').map_or(lower.len(), |e| start + e);
        from = end;
        let tag = &html[start..end];
        let rel = text::attribute(tag, "rel")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let kind = text::attribute(tag, "type")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !rel.split_whitespace().any(|r| r == "alternate") {
            continue;
        }
        if !(kind.contains("rss") || kind.contains("atom") || kind.contains("feed+json")) {
            continue;
        }
        let Some(href) = text::attribute(tag, "href") else {
            continue;
        };
        let resolved = match &base {
            Some(b) => b.join(&href).map(|u| u.to_string()).unwrap_or(href),
            None => href,
        };
        if !found.contains(&resolved) {
            found.push(resolved);
        }
    }
    found
}
