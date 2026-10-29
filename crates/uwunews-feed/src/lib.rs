//! Everything UwUNews does with a feed before it reaches the store.
//!
//! - [`parse`] turns RSS 0.9x/1.0/2.0, Atom and JSON Feed bytes into one
//!   shape, [`Feed`] with its [`Item`]s, and cleans what readers show.
//! - [`fetch`] asks a server for a feed, politely: with the ETag and
//!   Last-Modified of the last answer, so an unchanged feed costs a 304.
//! - [`discover`] finds the feed behind a web page someone pasted instead.
//! - [`filter`] decides whether an item belongs in a filter feed.
//! - [`opml`] reads and writes the subscription lists other readers export.
//!
//! Nothing here touches a database or a clock of its own: the store and the
//! app decide what is new and what was already announced.

pub mod discover;
pub mod fetch;
pub mod filter;
pub mod opml;
pub mod parse;
mod text;

pub use fetch::{Fetched, Fetcher, Validators};
pub use filter::{Rules, SearchIn};
pub use parse::{parse, Feed, Item};

/// What can go wrong between a URL and a parsed feed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a valid address: {0}")]
    Url(String),
    #[error("the server could not be reached: {0}")]
    Network(String),
    #[error("the server answered {0}")]
    Status(u16),
    #[error("the answer is larger than {0} bytes")]
    TooLarge(usize),
    #[error("not a feed: {0}")]
    NotAFeed(String),
    #[error("not an OPML file: {0}")]
    Opml(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A stable key for one item of one source: the store's primary key and what
/// read and starred marks sync under. The source id keeps two feeds that
/// reuse the same GUID apart.
pub fn item_key(source_id: &str, guid: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(source_id.as_bytes());
    hash.update([0]);
    hash.update(guid.as_bytes());
    hash.finalize()
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}
