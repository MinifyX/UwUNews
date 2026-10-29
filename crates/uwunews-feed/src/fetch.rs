//! Asking a server for a feed.
//!
//! Blocking on purpose: the app fetches from a background thread (or an iOS
//! background task) one source after another, and a feed is a few hundred
//! kilobytes at most. Every request carries the validators of the last
//! answer, so a feed that did not change costs the server a 304 and us
//! nothing.

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use crate::{parse, Error, Feed, Result};

/// The largest answer taken. Feeds that carry whole podcasts' show notes for
/// ten years get cut here rather than filling the phone.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

/// What the last answer said about itself, sent back on the next request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Validators {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Debug)]
pub enum Fetched {
    /// The feed did not change since the validators were given out.
    NotModified,
    Feed {
        feed: Feed,
        validators: Validators,
        /// Where the feed was in the end, after redirects.
        url: String,
    },
}

/// Raw bytes and what came with them, for callers that look before parsing
/// (feed discovery).
pub struct Answer {
    pub url: String,
    pub content_type: String,
    pub body: Vec<u8>,
    pub validators: Validators,
}

pub struct Fetcher {
    client: reqwest::blocking::Client,
}

impl Fetcher {
    pub fn new() -> Result<Self> {
        let roots: rustls::RootCertStore = webpki_roots::TLS_SERVER_ROOTS.iter().cloned().collect();
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::Network(e.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
        let client = reqwest::blocking::Client::builder()
            .tls_backend_preconfigured(tls)
            .user_agent(concat!(
                "UwUNews/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/MinifyX/UwUNews)"
            ))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(8))
            .build()
            .map_err(|e| Error::Network(e.to_string()))?;
        Ok(Self { client })
    }

    /// One GET, with the validators if there are any. `None` for a 304.
    pub fn get(&self, url: &str, validators: &Validators) -> Result<Option<Answer>> {
        let parsed = url::Url::parse(url).map_err(|e| Error::Url(e.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(Error::Url(format!(
                "{} is not a web address",
                parsed.scheme()
            )));
        }
        let mut request = self.client.get(parsed).header(
            "accept",
            "application/rss+xml, application/atom+xml, application/feed+json, application/xml;q=0.9, text/xml;q=0.9, */*;q=0.5",
        );
        if let Some(etag) = &validators.etag {
            request = request.header("if-none-match", etag);
        }
        if let Some(modified) = &validators.last_modified {
            request = request.header("if-modified-since", modified);
        }
        let response = request.send().map_err(|e| Error::Network(e.to_string()))?;
        let status = response.status().as_u16();
        if status == 304 {
            return Ok(None);
        }
        if !(200..300).contains(&status) {
            return Err(Error::Status(status));
        }
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let validators = Validators {
            etag: header("etag"),
            last_modified: header("last-modified"),
        };
        let content_type = header("content-type").unwrap_or_default();
        let url = response.url().to_string();
        let mut body = Vec::new();
        response
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut body)
            .map_err(|e| Error::Network(e.to_string()))?;
        if body.len() > MAX_BYTES {
            return Err(Error::TooLarge(MAX_BYTES));
        }
        Ok(Some(Answer {
            url,
            content_type,
            body,
            validators,
        }))
    }

    /// Fetches and parses a feed.
    pub fn fetch(&self, url: &str, validators: &Validators) -> Result<Fetched> {
        let Some(answer) = self.get(url, validators)? else {
            return Ok(Fetched::NotModified);
        };
        let feed = parse(&answer.body, Some(&answer.url))?;
        Ok(Fetched::Feed {
            feed,
            validators: answer.validators,
            url: answer.url,
        })
    }
}
