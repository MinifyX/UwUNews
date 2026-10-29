//! This device's cache of fetched articles, what the last fetch of each source
//! said, and which articles a notification already went out for. None of it
//! is synced: every device fetches its feeds itself, and only the read and
//! starred marks travel.

use crate::marks::article_key;
use crate::sync::{clock_of, supersede};
use crate::{now_ms, parse_uuid, Result, Store, StoreError};
use rusqlite::{params, params_from_iter, OptionalExtension};
use serde::Serialize;
use std::collections::HashMap;
use uuid::Uuid;
use uwunews_proto::EntityKind;

/// An item as a feed listed it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArticleDraft {
    /// The item's id within its feed (guid, Atom id, or its link).
    pub guid: String,
    pub title: String,
    pub link: Option<String>,
    pub summary: String,
    pub content_html: String,
    pub author: Option<String>,
    pub image_url: Option<String>,
    /// Unix milliseconds, when the feed said.
    pub published_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArticleRecord {
    pub key: String,
    pub source_id: Uuid,
    pub guid: String,
    pub title: String,
    pub link: Option<String>,
    pub summary: String,
    pub content_html: String,
    pub author: Option<String>,
    pub image_url: Option<String>,
    pub published_at: Option<i64>,
    pub fetched_at: i64,
    pub read: bool,
    pub starred: bool,
}

/// Which articles to list. Newest first: by publication time, or by when the
/// article was fetched for an item without one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArticleQuery {
    /// Only these sources; `None` for all.
    pub source_ids: Option<Vec<Uuid>>,
    pub unread_only: bool,
    pub starred_only: bool,
    /// Case-insensitive words, all of which must appear in the title or the
    /// summary.
    pub search: Option<String>,
    /// Only articles strictly older than this (unix ms) — to page on from the
    /// last one shown.
    pub before_published: Option<i64>,
    /// With `before_published`: the last key shown, so that articles at that
    /// same time but later in the order still come.
    pub before_key: Option<String>,
    /// Only articles at this time (unix ms) or newer.
    pub since: Option<i64>,
    /// Only the article with this key.
    pub key: Option<String>,
    /// At most this many; 0 for no limit.
    pub limit: usize,
}

/// What the last fetch of a source said. All times unix milliseconds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchState {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub last_attempt: Option<i64>,
    pub last_success: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneReport {
    pub articles: usize,
    pub marks: usize,
}

/// Pruned-article records older than this are forgotten: a feed that still
/// lists an item after a year has bigger problems.
const PRUNED_MEMORY_MS: i64 = 365 * 24 * 60 * 60 * 1000;

impl Store {
    /// Store what a fetch of `source_id` returned. Known articles are
    /// refreshed; the keys of the ones new here come back, in the order given.
    /// An article pruned here before is not brought back.
    pub fn upsert_articles(&self, source_id: Uuid, items: &[ArticleDraft]) -> Result<Vec<String>> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        if crate::sources::source_in(&tx, source_id)?.is_none() {
            return Err(StoreError::UnknownSource(source_id));
        }
        let now = now_ms() as i64;
        let source = source_id.to_string();
        let mut fresh = Vec::new();
        {
            let mut pruned = tx.prepare("SELECT 1 FROM pruned_articles WHERE key = ?1")?;
            let mut insert = tx.prepare(
                "INSERT INTO articles
                    (key, source_id, guid, title, link, summary, content_html,
                     author, image_url, published_at, fetched_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT (key) DO NOTHING",
            )?;
            let mut update = tx.prepare(
                "UPDATE articles
                    SET title = ?2, link = ?3, summary = ?4, content_html = ?5,
                        author = ?6, image_url = ?7,
                        published_at = coalesce(?8, published_at)
                  WHERE key = ?1",
            )?;
            for item in items {
                let guid = item.guid.trim();
                if guid.is_empty() {
                    continue;
                }
                let key = article_key(source_id, guid);
                if pruned.exists([&key])? {
                    continue;
                }
                let inserted = insert.execute(params![
                    key,
                    source,
                    guid,
                    item.title,
                    item.link,
                    item.summary,
                    item.content_html,
                    item.author,
                    item.image_url,
                    item.published_at,
                    now,
                ])?;
                if inserted == 1 {
                    if !fresh.contains(&key) {
                        fresh.push(key);
                    }
                } else {
                    update.execute(params![
                        key,
                        item.title,
                        item.link,
                        item.summary,
                        item.content_html,
                        item.author,
                        item.image_url,
                        item.published_at,
                    ])?;
                }
            }
        }
        tx.commit()?;
        Ok(fresh)
    }

    pub fn articles(&self, query: &ArticleQuery) -> Result<Vec<ArticleRecord>> {
        let Some((filter, mut args)) = filter(query) else {
            return Ok(Vec::new());
        };
        let mut sql = format!(
            "SELECT a.key, a.source_id, a.guid, a.title, a.link, a.summary,
                    a.content_html, a.author, a.image_url, a.published_at,
                    a.fetched_at, coalesce(m.read, 0), coalesce(m.starred, 0){filter}"
        );
        sql.push_str(" ORDER BY coalesce(a.published_at, a.fetched_at) DESC, a.key");
        if query.limit > 0 {
            sql.push_str(" LIMIT ?");
            args.push((query.limit as i64).into());
        }
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(args), |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    ArticleRecord {
                        key: row.get(0)?,
                        source_id: Uuid::nil(),
                        guid: row.get(2)?,
                        title: row.get(3)?,
                        link: row.get(4)?,
                        summary: row.get(5)?,
                        content_html: row.get(6)?,
                        author: row.get(7)?,
                        image_url: row.get(8)?,
                        published_at: row.get(9)?,
                        fetched_at: row.get(10)?,
                        read: row.get(11)?,
                        starred: row.get(12)?,
                    },
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(source, mut article)| {
                article.source_id = parse_uuid(&source)?;
                Ok(article)
            })
            .collect()
    }

    /// How many articles `query` would list, its limit aside.
    pub fn count_articles(&self, query: &ArticleQuery) -> Result<usize> {
        let Some((filter, args)) = filter(query) else {
            return Ok(0);
        };
        let conn = self.conn.lock();
        let n: i64 = conn.query_row(
            &format!("SELECT count(*){filter}"),
            params_from_iter(args),
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }

    pub fn article(&self, key: &str) -> Result<Option<ArticleRecord>> {
        let conn = self.conn.lock();
        let source: Option<String> = conn
            .query_row(
                "SELECT source_id FROM articles WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()?;
        drop(conn);
        let Some(source) = source else {
            return Ok(None);
        };
        Ok(self
            .articles(&ArticleQuery {
                source_ids: Some(vec![parse_uuid(&source)?]),
                key: Some(key.to_string()),
                ..ArticleQuery::default()
            })?
            .into_iter()
            .find(|a| a.key == key))
    }

    /// Unread articles per source; sources with none are left out.
    pub fn unread_counts(&self) -> Result<HashMap<Uuid, usize>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT a.source_id, count(*)
               FROM articles a
               LEFT JOIN news_marks m ON m.article = a.key AND m.deleted = 0
              WHERE coalesce(m.read, 0) = 0
              GROUP BY a.source_id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, n)| Ok((parse_uuid(&id)?, n as usize)))
            .collect()
    }

    pub fn fetch_state(&self, source_id: Uuid) -> Result<Option<FetchState>> {
        Ok(self
            .conn
            .lock()
            .query_row(
                "SELECT etag, last_modified, last_attempt, last_success, last_error
                   FROM fetch_state WHERE source_id = ?1",
                [source_id.to_string()],
                |row| {
                    Ok(FetchState {
                        etag: row.get(0)?,
                        last_modified: row.get(1)?,
                        last_attempt: row.get(2)?,
                        last_success: row.get(3)?,
                        last_error: row.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn set_fetch_state(&self, source_id: Uuid, state: &FetchState) -> Result<()> {
        self.conn.lock().execute(
            "INSERT INTO fetch_state
                (source_id, etag, last_modified, last_attempt, last_success, last_error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (source_id) DO UPDATE SET
                etag = excluded.etag, last_modified = excluded.last_modified,
                last_attempt = excluded.last_attempt,
                last_success = excluded.last_success,
                last_error = excluded.last_error",
            params![
                source_id.to_string(),
                state.etag,
                state.last_modified,
                state.last_attempt,
                state.last_success,
                state.last_error
            ],
        )?;
        Ok(())
    }

    /// Of these articles, the ones no notification went out for yet — and
    /// from now on, one has. Two callers racing get disjoint answers.
    pub fn take_unnotified(&self, keys: &[String]) -> Result<Vec<String>> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut taken = Vec::new();
        {
            let mut insert = tx.prepare("INSERT OR IGNORE INTO notified (key) VALUES (?1)")?;
            for key in keys {
                if insert.execute([key])? == 1 {
                    taken.push(key.clone());
                }
            }
        }
        tx.commit()?;
        Ok(taken)
    }

    /// Drop articles published (or, without a date, fetched) before
    /// `older_than_ms`, unix ms — starred ones stay. Marks of that age that
    /// are not starred go too, locally and without a tombstone: other devices
    /// keep theirs until they prune. A mark not yet pushed stays until it is.
    pub fn prune(&self, older_than_ms: i64) -> Result<PruneReport> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let now = now_ms() as i64;
        let old = "SELECT a.key FROM articles a
                     LEFT JOIN news_marks m ON m.article = a.key AND m.deleted = 0
                    WHERE coalesce(a.published_at, a.fetched_at) < ?1
                      AND coalesce(m.starred, 0) = 0";
        tx.execute(
            &format!(
                "INSERT OR REPLACE INTO pruned_articles (key, source_id, pruned_at)
                 SELECT key, source_id, ?2 FROM articles WHERE key IN ({old})"
            ),
            params![older_than_ms, now],
        )?;
        tx.execute(
            "DELETE FROM notified WHERE key IN (SELECT key FROM pruned_articles WHERE pruned_at = ?1)",
            [now],
        )?;
        let articles = tx.execute(
            "DELETE FROM articles WHERE key IN (SELECT key FROM pruned_articles WHERE pruned_at = ?1)",
            [now],
        )?;
        tx.execute(
            "DELETE FROM pruned_articles WHERE pruned_at < ?1",
            [now - PRUNED_MEMORY_MS],
        )?;
        let marks: Vec<(String, i64, i64, i64)> = {
            let mut stmt = tx.prepare(
                "SELECT id, hlc_wall_ms, hlc_counter, hlc_device FROM news_marks
                  WHERE dirty = 0 AND starred = 0
                    AND coalesce(published_at, hlc_wall_ms) < ?1",
            )?;
            let rows = stmt
                .query_map([older_than_ms], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        for (id, wall, counter, device) in &marks {
            supersede(
                &tx,
                EntityKind::NewsMark,
                parse_uuid(id)?,
                clock_of(*wall, *counter, *device),
            )?;
            tx.execute("DELETE FROM news_marks WHERE id = ?1", [id])?;
        }
        tx.commit()?;
        Ok(PruneReport {
            articles,
            marks: marks.len(),
        })
    }
}

/// The `FROM` and `WHERE` part of an article query, with its arguments; `None`
/// when nothing can match.
fn filter(query: &ArticleQuery) -> Option<(String, Vec<rusqlite::types::Value>)> {
    let mut sql = String::from(
        " FROM articles a
      LEFT JOIN news_marks m ON m.article = a.key AND m.deleted = 0
     WHERE 1 = 1",
    );
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(ids) = &query.source_ids {
        if ids.is_empty() {
            return None;
        }
        sql.push_str(" AND a.source_id IN (");
        for (i, id) in ids.iter().enumerate() {
            sql.push_str(if i == 0 { "?" } else { ", ?" });
            args.push(id.to_string().into());
        }
        sql.push(')');
    }
    if query.unread_only {
        sql.push_str(" AND coalesce(m.read, 0) = 0");
    }
    if query.starred_only {
        sql.push_str(" AND coalesce(m.starred, 0) = 1");
    }
    if let Some(search) = &query.search {
        for word in search.split_whitespace() {
            let pattern = format!(
                "%{}%",
                word.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            );
            sql.push_str(" AND (a.title LIKE ? ESCAPE '\\' OR a.summary LIKE ? ESCAPE '\\')");
            args.push(pattern.clone().into());
            args.push(pattern.into());
        }
    }
    match (query.before_published, &query.before_key) {
        (Some(before), Some(key)) => {
            sql.push_str(
                " AND (coalesce(a.published_at, a.fetched_at) < ? \
                 OR (coalesce(a.published_at, a.fetched_at) = ? AND a.key > ?))",
            );
            args.push(before.into());
            args.push(before.into());
            args.push(key.clone().into());
        }
        (Some(before), None) => {
            sql.push_str(" AND coalesce(a.published_at, a.fetched_at) < ?");
            args.push(before.into());
        }
        _ => {}
    }
    if let Some(since) = query.since {
        sql.push_str(" AND coalesce(a.published_at, a.fetched_at) >= ?");
        args.push(since.into());
    }
    if let Some(key) = &query.key {
        sql.push_str(" AND a.key = ?");
        args.push(key.clone().into());
    }
    Some((sql, args))
}
