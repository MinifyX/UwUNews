//! What UwUNews stores and syncs.
//!
//! One record is an [`Envelope`](crate::Envelope) — id, kind, clock, tombstone
//! — plus a sealed payload. The payloads live here, and they hold *only* the
//! fields that mean something on another device: no fetched articles, no
//! validators, no notification bookkeeping. Those are this device's cache and
//! stay in its own tables.
//!
//! UwUNews shares its account — and so its server and its vault — with UwUSSH
//! or UwURDP. Their kinds take the discriminants from 0 up; UwUNews starts its
//! own at [`EntityKind::NewsSource`] = 40, far enough from theirs that neither
//! side will ever reach the other's. Records of a kind a build does not know
//! are skipped on the way in (see [`crate::sync`]), and a manifest entry of
//! such a kind is not checked, so the apps can share one vault without
//! understanding each other's records.
//!
//! Every payload keeps the fields it did not recognise in [`Extra`]. A device
//! running an older build can therefore edit a collection a newer one wrote
//! without dropping the fields it has never heard of.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Fields a payload did not recognise, kept verbatim so they survive an edit
/// on a device whose build is older than the one that wrote them.
pub type Extra = serde_json::Map<String, serde_json::Value>;

fn extra_is_empty(extra: &Extra) -> bool {
    extra.is_empty()
}

/// What kind of record a blob holds. Part of the associated data when
/// encrypting, so a malicious server cannot hand back a collection where a
/// source was expected.
///
/// **Append only.** The discriminant is what goes into the associated data;
/// changing one would make every sealed record of that kind unreadable. They
/// are written out so that reordering the list can't do it by accident.
///
/// `Manifest` is the one kind shared with UwUSSH and UwURDP: same
/// discriminant, same name, same binary payload. 0–39 belong to those apps and
/// must never be used here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum EntityKind {
    /// What one device holds: every record's id and version, so another
    /// device can tell whether the server handed it everything. See
    /// [`crate::manifest`].
    Manifest = 9,
    /// A feed this account follows.
    NewsSource = 40,
    /// A user-made view over sources, with keyword rules.
    NewsCollection = 41,
    /// Read and starred, for one article.
    NewsMark = 42,
}

impl EntityKind {
    /// Every kind this build knows, in discriminant order.
    pub const ALL: [Self; 4] = [
        Self::Manifest,
        Self::NewsSource,
        Self::NewsCollection,
        Self::NewsMark,
    ];

    /// The kind a discriminant stands for, or `None` for one this build does
    /// not know — another app's, or a newer build's.
    pub fn from_discriminant(value: u8) -> Option<Self> {
        Some(match value {
            9 => Self::Manifest,
            40 => Self::NewsSource,
            41 => Self::NewsCollection,
            42 => Self::NewsMark,
            _ => return None,
        })
    }

    /// Kinds a record can point at, before the kinds that point at them.
    ///
    /// Manifests are not in here on purpose: this list doubles as the list of
    /// record tables, and a manifest sorts last anyway, after the records it
    /// talks about.
    pub const APPLY_ORDER: [Self; 3] = [Self::NewsSource, Self::NewsCollection, Self::NewsMark];

    /// Where this kind sorts when a batch is applied.
    pub fn apply_rank(self) -> usize {
        Self::APPLY_ORDER
            .iter()
            .position(|kind| *kind == self)
            .unwrap_or(Self::APPLY_ORDER.len())
    }
}

/// A feed, as another device needs it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsSourcePayload {
    /// The feed's own address, after discovery.
    pub url: String,
    /// The name the user gave it. `None` shows the feed's own title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The site the feed belongs to, for its icon and the "open site" link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site_url: Option<String>,
    /// Whether new articles here raise a notification.
    #[serde(default)]
    pub notify: bool,
    /// Unix milliseconds.
    #[serde(default)]
    pub created_at: i64,
    #[serde(flatten, default, skip_serializing_if = "extra_is_empty")]
    pub extra: Extra,
}

/// A collection: some sources, or all of them, filtered by keywords.
///
/// `symbol`, `color` and `search_in` are strings rather than enums on purpose:
/// a build that meets a value it does not know shows a default instead of
/// refusing the whole record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsCollectionPayload {
    pub name: String,
    /// An SF Symbol name.
    #[serde(default)]
    pub symbol: String,
    /// `pink`, `violet`, `sky`, `mint`, `amber` or `coral`.
    #[serde(default)]
    pub color: String,
    /// Every source, now and later; `source_ids` is ignored then.
    #[serde(default)]
    pub all_sources: bool,
    #[serde(default)]
    pub source_ids: Vec<Uuid>,
    #[serde(default)]
    pub rules: NewsRulesPayload,
    #[serde(default)]
    pub notify: bool,
    /// Where it sorts in the sidebar, ascending.
    #[serde(default)]
    pub sort_index: i64,
    #[serde(flatten, default, skip_serializing_if = "extra_is_empty")]
    pub extra: Extra,
}

/// A collection's keyword rules. Empty lists mean "no rule".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NewsRulesPayload {
    /// At least one of these must match.
    pub any_keywords: Vec<String>,
    /// Every one of these must match.
    pub all_keywords: Vec<String>,
    /// None of these may match.
    pub exclude_keywords: Vec<String>,
    /// `title` or `title_and_content`.
    pub search_in: String,
    #[serde(flatten, skip_serializing_if = "extra_is_empty")]
    pub extra: Extra,
}

/// Read and starred for one article.
///
/// The record id is derived from the article key (see the store), so two
/// devices that mark the same article write the same record and the newer
/// mark wins, instead of producing two.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsMarkPayload {
    /// The article key: hex SHA-256 over the source id and the item's guid.
    pub article: String,
    pub source_id: Uuid,
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub starred: bool,
    /// When the article was published, unix milliseconds — so a device that
    /// never fetched the article can still tell when the mark may be pruned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<i64>,
    #[serde(flatten, default, skip_serializing_if = "extra_is_empty")]
    pub extra: Extra,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_from_a_newer_build_survive_a_round_trip() {
        let written = r#"{"name":"Rust","symbol":"gear","color":"mint","source_ids":[],
            "rules":{"any_keywords":["rust"],"search_in":"title","regex":"r.st"},
            "sort_index":2,"pinned":true}"#;
        let collection: NewsCollectionPayload = serde_json::from_str(written).unwrap();
        assert_eq!(collection.extra["pinned"], true);
        assert_eq!(collection.rules.extra["regex"], "r.st");
        let back = serde_json::to_value(&collection).unwrap();
        assert_eq!(back["pinned"], true);
        assert_eq!(back["rules"]["regex"], "r.st");
    }

    #[test]
    fn a_minimal_record_reads_with_defaults() {
        let source: NewsSourcePayload =
            serde_json::from_str(r#"{"url":"https://news.example/feed"}"#).unwrap();
        assert!(!source.notify);
        assert_eq!(source.title, None);
        let collection: NewsCollectionPayload = serde_json::from_str(r#"{"name":"A"}"#).unwrap();
        assert!(collection.rules.any_keywords.is_empty());
        let mark: NewsMarkPayload = serde_json::from_str(
            r#"{"article":"ab","source_id":"00000000-0000-0000-0000-000000000001"}"#,
        )
        .unwrap();
        assert!(!mark.read && !mark.starred);
    }

    #[test]
    fn the_discriminants_never_move() {
        // They go into the associated data of every sealed record, so a
        // reordering here would make existing vaults unreadable.
        assert_eq!(EntityKind::Manifest as u8, 9);
        assert_eq!(EntityKind::NewsSource as u8, 40);
        assert_eq!(EntityKind::NewsCollection as u8, 41);
        assert_eq!(EntityKind::NewsMark as u8, 42);
        for kind in EntityKind::ALL {
            assert_eq!(EntityKind::from_discriminant(kind as u8), Some(kind));
        }
        // UwUSSH's and UwURDP's kinds are not this build's.
        for other in (0..9).chain(10..40) {
            assert_eq!(EntityKind::from_discriminant(other), None);
        }
    }

    #[test]
    fn the_wire_names_are_the_ones_the_server_stores() {
        let names: Vec<String> = EntityKind::ALL
            .iter()
            .map(|kind| serde_json::to_value(kind).unwrap().as_str().unwrap().into())
            .collect();
        assert_eq!(
            names,
            ["manifest", "news_source", "news_collection", "news_mark"]
        );
        // Another app's kind is not one of ours.
        assert!(serde_json::from_str::<EntityKind>(r#""host""#).is_err());
    }

    #[test]
    fn a_manifest_is_applied_after_everything_it_lists() {
        for kind in EntityKind::APPLY_ORDER {
            assert!(kind.apply_rank() < EntityKind::Manifest.apply_rank());
        }
        assert!(EntityKind::NewsSource.apply_rank() < EntityKind::NewsCollection.apply_rank());
    }
}
