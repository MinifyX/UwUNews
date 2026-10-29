//! Read and starred, one synced `news_mark` per article.
//!
//! A mark's id is derived from the article key, and the article key from the
//! source id and the item's guid, so every device that marks the same article
//! writes the same record and the newer write wins.

use crate::{hex, tick, vault_id, Result, Store, StoreError};
use rusqlite::{params, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// The key of an article: hex SHA-256 over
/// `"uwunews/article/v1\0" + source id (hyphenated, lowercase) + "\0" + guid`.
pub fn article_key(source_id: Uuid, guid: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"uwunews/article/v1\0");
    hash.update(source_id.hyphenated().to_string().as_bytes());
    hash.update(b"\0");
    hash.update(guid.as_bytes());
    hex(&hash.finalize())
}

/// The id of the mark for an article: a version-8 UUID from SHA-256 over
/// `"uwunews/mark/v1" + article key`.
pub fn mark_id(article_key: &str) -> Uuid {
    let mut hash = Sha256::new();
    hash.update(b"uwunews/mark/v1");
    hash.update(article_key.as_bytes());
    let digest = hash.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_custom_bytes(bytes).into_uuid()
}

#[derive(Clone, Copy)]
enum Flag {
    Read,
    Starred,
}

impl Flag {
    fn column(self) -> &'static str {
        match self {
            Flag::Read => "read",
            Flag::Starred => "starred",
        }
    }
}

impl Store {
    pub fn set_read(&self, article: &str, read: bool) -> Result<()> {
        self.set_flag(article, Flag::Read, read)
    }

    pub fn set_starred(&self, article: &str, starred: bool) -> Result<()> {
        self.set_flag(article, Flag::Starred, starred)
    }

    fn set_flag(&self, article: &str, flag: Flag, value: bool) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let fetched: Option<(String, Option<i64>)> = tx
            .query_row(
                "SELECT source_id, published_at FROM articles WHERE key = ?1",
                [article],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (source_id, published_at) = match fetched {
            Some(found) => found,
            None => tx
                .query_row(
                    "SELECT source_id, published_at FROM news_marks WHERE article = ?1",
                    [article],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?
                .ok_or_else(|| StoreError::UnknownArticle(article.to_string()))?,
        };
        write_mark(
            &tx,
            self.device,
            article,
            &source_id,
            published_at,
            flag,
            value,
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Mark every unread article of these sources (or of all) read.
    pub fn mark_all_read(&self, sources: Option<Vec<Uuid>>) -> Result<usize> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let unread: Vec<(String, String, Option<i64>)> = {
            let mut stmt = tx.prepare(
                "SELECT a.key, a.source_id, a.published_at
                   FROM articles a
                   LEFT JOIN news_marks m ON m.article = a.key AND m.deleted = 0
                  WHERE coalesce(m.read, 0) = 0",
            )?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        let wanted: Option<Vec<String>> =
            sources.map(|ids| ids.iter().map(Uuid::to_string).collect());
        let mut count = 0;
        for (key, source_id, published_at) in unread {
            if let Some(wanted) = &wanted {
                if !wanted.contains(&source_id) {
                    continue;
                }
            }
            write_mark(
                &tx,
                self.device,
                &key,
                &source_id,
                published_at,
                Flag::Read,
                true,
            )?;
            count += 1;
        }
        tx.commit()?;
        Ok(count)
    }
}

fn write_mark(
    tx: &Transaction,
    device: u32,
    article: &str,
    source_id: &str,
    published_at: Option<i64>,
    flag: Flag,
    value: bool,
) -> Result<()> {
    let id = mark_id(article).to_string();
    let existing: Option<(bool, bool, bool)> = tx
        .query_row(
            "SELECT read, starred, deleted FROM news_marks WHERE id = ?1",
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let column = flag.column();
    match existing {
        // Nothing to clear.
        None if !value => Ok(()),
        Some((_, _, true)) if !value => Ok(()),
        Some((read, starred, false))
            if match flag {
                Flag::Read => read == value,
                Flag::Starred => starred == value,
            } =>
        {
            Ok(())
        }
        None => {
            let clock = tick(tx, device)?;
            tx.execute(
                &format!(
                    "INSERT INTO news_marks
                        (id, vault_id, article, source_id, {column}, published_at,
                         hlc_wall_ms, hlc_counter, hlc_device)
                     VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7, ?8)"
                ),
                params![
                    id,
                    vault_id(tx)?,
                    article,
                    source_id,
                    published_at,
                    clock.wall_ms as i64,
                    clock.counter,
                    clock.device
                ],
            )?;
            Ok(())
        }
        Some((_, _, deleted)) => {
            let clock = tick(tx, device)?;
            // A tombstoned mark comes back with only this flag set.
            let reset = if deleted {
                "read = 0, starred = 0, deleted = 0,"
            } else {
                ""
            };
            tx.execute(
                &format!(
                    "UPDATE news_marks
                        SET {reset} {column} = ?2,
                            published_at = coalesce(published_at, ?3),
                            rev = rev + 1, dirty = 1,
                            hlc_wall_ms = ?4, hlc_counter = ?5, hlc_device = ?6
                      WHERE id = ?1"
                ),
                params![
                    id,
                    value,
                    published_at,
                    clock.wall_ms as i64,
                    clock.counter,
                    clock.device
                ],
            )?;
            Ok(())
        }
    }
}
