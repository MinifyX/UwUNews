//! One shape for every feed format.
//!
//! `feed-rs` does the format work (RSS 0.9x/1.0/2.0, Atom, JSON Feed). This
//! module picks what a reader needs out of its model and makes it safe to
//! show: titles as plain text, a short summary, relative links resolved
//! against the feed, and an image if the item names or contains one.

use crate::{text, Error, Result};

/// A feed as UwUNews keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feed {
    pub title: String,
    /// The web site the feed belongs to, if it says.
    pub site_url: Option<String>,
    pub items: Vec<Item>,
}

/// One article, episode or post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The feed's own id for it, or one `feed-rs` derived from its link and
    /// title when the feed gives none. Stable across fetches.
    pub guid: String,
    pub title: String,
    pub link: Option<String>,
    /// Plain text, one or two lines' worth.
    pub summary: String,
    /// The full HTML the feed carries, for the reader. Empty if none.
    pub content_html: String,
    pub author: Option<String>,
    pub image_url: Option<String>,
    /// Unix milliseconds: published, or updated when that is all there is.
    pub published_at: Option<i64>,
}

/// How long a summary gets.
pub const SUMMARY_CHARS: usize = 280;
/// More items than this in one feed are cut, newest first.
pub const MAX_ITEMS: usize = 500;

/// Parses a feed. `url` is where it came from, for resolving relative links.
pub fn parse(bytes: &[u8], url: Option<&str>) -> Result<Feed> {
    let parser = feed_rs::parser::Builder::new().base_uri(url).build();
    let feed = parser
        .parse(bytes)
        .map_err(|e| Error::NotAFeed(e.to_string()))?;
    let base = url.and_then(|u| url::Url::parse(u).ok());

    let site_url = feed
        .links
        .iter()
        .find(|l| l.rel.as_deref().is_none_or(|r| r == "alternate"))
        .and_then(|l| resolve(base.as_ref(), &l.href));
    let title = feed
        .title
        .map(|t| clean_title(&t.content))
        .filter(|t| !t.is_empty())
        .or_else(|| site_url.as_deref().and_then(host_of))
        .or_else(|| url.and_then(host_of))
        .unwrap_or_default();

    let mut items: Vec<Item> = feed
        .entries
        .into_iter()
        .map(|entry| {
            let link = entry
                .links
                .iter()
                .find(|l| l.rel.as_deref().is_none_or(|r| r == "alternate"))
                .or(entry.links.first())
                .and_then(|l| resolve(base.as_ref(), &l.href));
            let content_html = entry
                .content
                .as_ref()
                .and_then(|c| c.body.clone())
                .unwrap_or_default();
            let summary_html = entry
                .summary
                .as_ref()
                .map(|s| s.content.clone())
                .unwrap_or_default();
            let summary = if summary_html.trim().is_empty() {
                text::plain(&content_html, SUMMARY_CHARS)
            } else {
                text::plain(&summary_html, SUMMARY_CHARS)
            };
            // A feed with only a summary still gets something in the reader.
            let content_html = if content_html.trim().is_empty() {
                summary_html.clone()
            } else {
                content_html
            };
            let image_url = entry
                .media
                .iter()
                .flat_map(|m| {
                    m.thumbnails.iter().map(|t| t.image.uri.clone()).chain(
                        m.content.iter().filter_map(|c| {
                            let image = c
                                .content_type
                                .as_ref()
                                .is_none_or(|t| t.as_str().starts_with("image/"));
                            c.url.as_ref().filter(|_| image).map(|u| u.to_string())
                        }),
                    )
                })
                .find(|u| u.starts_with("http"))
                .or_else(|| text::first_image(&content_html))
                .or_else(|| text::first_image(&summary_html))
                .and_then(|u| resolve(base.as_ref(), &u));
            let title = entry
                .title
                .map(|t| clean_title(&t.content))
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| text::cut(&summary, 80));
            Item {
                guid: entry.id,
                title,
                link,
                summary,
                content_html,
                author: entry
                    .authors
                    .first()
                    .and_then(|a| a.name.as_deref())
                    .map(|a| a.trim().to_string())
                    .filter(|a| !a.is_empty()),
                image_url,
                published_at: entry
                    .published
                    .or(entry.updated)
                    .map(|d| d.timestamp_millis()),
            }
        })
        .collect();
    items.sort_by_key(|item| std::cmp::Reverse(item.published_at));
    items.truncate(MAX_ITEMS);
    Ok(Feed {
        title,
        site_url,
        items,
    })
}

/// Titles come as text in some feeds and as HTML in others.
fn clean_title(title: &str) -> String {
    text::plain(title, 300)
}

/// A link made absolute against the feed's address. Only web links: a feed
/// must not be able to hand out `shortcuts:` or another app's scheme.
fn resolve(base: Option<&url::Url>, href: &str) -> Option<String> {
    let url = match base {
        Some(base) => base.join(href).ok()?,
        None => url::Url::parse(href).ok()?,
    };
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()?
        .host_str()
        .map(|h| h.trim_start_matches("www.").to_string())
}
