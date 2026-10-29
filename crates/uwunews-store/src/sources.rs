//! The feeds this account follows. Synced as `news_source`.

use crate::{now_ms, parse_uuid, tick, vault_id, Result, Store, StoreError};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::Serialize;
use uuid::Uuid;

/// A feed to follow.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceDraft {
    /// The feed's own address, after discovery.
    pub url: String,
    /// The name the user gave it; `None` shows the feed's own title.
    pub title: Option<String>,
    pub site_url: Option<String>,
    pub notify: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRecord {
    pub id: Uuid,
    pub url: String,
    pub title: Option<String>,
    pub site_url: Option<String>,
    pub notify: bool,
    /// Unix milliseconds.
    pub created_at: i64,
}

const COLUMNS: &str = "id, url, title, site_url, notify, created_at";

fn record(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, SourceRecord)> {
    Ok((
        row.get(0)?,
        SourceRecord {
            id: Uuid::nil(),
            url: row.get(1)?,
            title: row.get(2)?,
            site_url: row.get(3)?,
            notify: row.get(4)?,
            created_at: row.get(5)?,
        },
    ))
}

fn with_id((id, mut source): (String, SourceRecord)) -> Result<SourceRecord> {
    source.id = parse_uuid(&id)?;
    Ok(source)
}

fn clean(text: Option<String>) -> Option<String> {
    text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

impl Store {
    /// Follow a feed. The same address twice is refused.
    pub fn add_source(&self, draft: SourceDraft) -> Result<SourceRecord> {
        let url = draft.url.trim().to_string();
        if url.is_empty() {
            return Err(StoreError::Invalid {
                field: "url",
                problem: "empty",
            });
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let taken: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM news_sources WHERE url = ?1 AND deleted = 0",
                [&url],
                |row| row.get(0),
            )
            .optional()?;
        if taken.is_some() {
            return Err(StoreError::DuplicateSource);
        }
        let id = Uuid::now_v7();
        let clock = tick(&tx, self.device)?;
        let source = SourceRecord {
            id,
            url,
            title: clean(draft.title),
            site_url: clean(draft.site_url),
            notify: draft.notify,
            created_at: now_ms() as i64,
        };
        tx.execute(
            "INSERT INTO news_sources
                (id, vault_id, url, title, site_url, notify, created_at,
                 hlc_wall_ms, hlc_counter, hlc_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id.to_string(),
                vault_id(&tx)?,
                source.url,
                source.title,
                source.site_url,
                source.notify,
                source.created_at,
                clock.wall_ms as i64,
                clock.counter,
                clock.device,
            ],
        )?;
        tx.commit()?;
        Ok(source)
    }

    /// Rename a source (`None` goes back to the feed's own title) and switch
    /// its notifications.
    pub fn update_source(
        &self,
        id: Uuid,
        title: Option<String>,
        notify: bool,
    ) -> Result<SourceRecord> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let clock = tick(&tx, self.device)?;
        let changed = tx.execute(
            "UPDATE news_sources
                SET title = ?2, notify = ?3, rev = rev + 1, dirty = 1,
                    hlc_wall_ms = ?4, hlc_counter = ?5, hlc_device = ?6
              WHERE id = ?1 AND deleted = 0",
            params![
                id.to_string(),
                clean(title),
                notify,
                clock.wall_ms as i64,
                clock.counter,
                clock.device
            ],
        )?;
        if changed == 0 {
            return Err(StoreError::UnknownSource(id));
        }
        let source = source_in(&tx, id)?.ok_or(StoreError::UnknownSource(id))?;
        tx.commit()?;
        Ok(source)
    }

    /// Remember the site a feed belongs to, once a fetch has said. Synced, but
    /// only written when it changed.
    pub fn set_source_site_url(&self, id: Uuid, site_url: Option<String>) -> Result<()> {
        let site_url = clean(site_url);
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let current = source_in(&tx, id)?.ok_or(StoreError::UnknownSource(id))?;
        if current.site_url == site_url {
            return Ok(());
        }
        let clock = tick(&tx, self.device)?;
        tx.execute(
            "UPDATE news_sources
                SET site_url = ?2, rev = rev + 1, dirty = 1,
                    hlc_wall_ms = ?3, hlc_counter = ?4, hlc_device = ?5
              WHERE id = ?1",
            params![
                id.to_string(),
                site_url,
                clock.wall_ms as i64,
                clock.counter,
                clock.device
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Stop following a feed: a tombstone for the source, its articles and
    /// fetch state dropped, and it is taken out of every collection that
    /// named it.
    pub fn delete_source(&self, id: Uuid) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let clock = tick(&tx, self.device)?;
        let changed = tx.execute(
            "UPDATE news_sources
                SET deleted = 1, rev = rev + 1, dirty = 1,
                    hlc_wall_ms = ?2, hlc_counter = ?3, hlc_device = ?4
              WHERE id = ?1 AND deleted = 0",
            params![
                id.to_string(),
                clock.wall_ms as i64,
                clock.counter,
                clock.device
            ],
        )?;
        if changed == 0 {
            return Err(StoreError::UnknownSource(id));
        }
        drop_source_cache(&tx, id)?;
        crate::collections::forget_source(&tx, self.device, id)?;
        tx.commit()?;
        Ok(())
    }

    /// Every source followed, by title (or address).
    pub fn list_sources(&self) -> Result<Vec<SourceRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM news_sources WHERE deleted = 0
              ORDER BY lower(coalesce(title, url)), id"
        ))?;
        let rows = stmt
            .query_map([], record)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(with_id).collect()
    }

    pub fn source(&self, id: Uuid) -> Result<Option<SourceRecord>> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        source_in(&tx, id)
    }
}

pub(crate) fn source_in(tx: &Transaction, id: Uuid) -> Result<Option<SourceRecord>> {
    tx.query_row(
        &format!("SELECT {COLUMNS} FROM news_sources WHERE id = ?1 AND deleted = 0"),
        [id.to_string()],
        record,
    )
    .optional()?
    .map(with_id)
    .transpose()
}

/// What this device fetched for a source, gone with it.
pub(crate) fn drop_source_cache(tx: &Transaction, id: Uuid) -> Result<()> {
    let id = id.to_string();
    tx.execute(
        "DELETE FROM notified WHERE key IN (SELECT key FROM articles WHERE source_id = ?1)",
        [&id],
    )?;
    tx.execute("DELETE FROM articles WHERE source_id = ?1", [&id])?;
    tx.execute("DELETE FROM pruned_articles WHERE source_id = ?1", [&id])?;
    tx.execute("DELETE FROM fetch_state WHERE source_id = ?1", [&id])?;
    Ok(())
}
