//! Collections: some sources, or all of them, filtered by keywords. Synced as
//! `news_collection`.

use crate::{parse_uuid, tick, vault_id, Result, Store, StoreError};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::Serialize;
use uuid::Uuid;
use uwunews_proto::NewsRulesPayload;

/// A collection's keyword rules. Empty lists mean "no rule".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionRules {
    pub any_keywords: Vec<String>,
    pub all_keywords: Vec<String>,
    pub exclude_keywords: Vec<String>,
    /// `title` or `title_and_content`; anything else reads as `title`.
    pub search_in: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CollectionDraft {
    pub name: String,
    /// An SF Symbol name.
    pub symbol: String,
    pub color: String,
    /// Every source, now and later; `source_ids` is ignored then.
    pub all_sources: bool,
    pub source_ids: Vec<Uuid>,
    pub rules: CollectionRules,
    pub notify: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionRecord {
    pub id: Uuid,
    pub name: String,
    pub symbol: String,
    pub color: String,
    pub all_sources: bool,
    pub source_ids: Vec<Uuid>,
    pub rules: CollectionRules,
    pub notify: bool,
    pub sort_index: i64,
}

impl From<NewsRulesPayload> for CollectionRules {
    fn from(rules: NewsRulesPayload) -> Self {
        Self {
            any_keywords: rules.any_keywords,
            all_keywords: rules.all_keywords,
            exclude_keywords: rules.exclude_keywords,
            search_in: rules.search_in,
        }
    }
}

fn clean_words(words: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in words {
        let word = word.trim().to_string();
        if !word.is_empty() && !out.iter().any(|w| w.eq_ignore_ascii_case(&word)) {
            out.push(word);
        }
    }
    out
}

/// The rules as stored: the new ones, with the fields a newer build wrote
/// into the old ones kept.
fn rules_json(rules: CollectionRules, old: Option<&str>) -> Result<String> {
    let extra = old
        .and_then(|text| serde_json::from_str::<NewsRulesPayload>(text).ok())
        .map(|old| old.extra)
        .unwrap_or_default();
    let payload = NewsRulesPayload {
        any_keywords: clean_words(rules.any_keywords),
        all_keywords: clean_words(rules.all_keywords),
        exclude_keywords: clean_words(rules.exclude_keywords),
        search_in: rules.search_in.trim().to_string(),
        extra,
    };
    serde_json::to_string(&payload).map_err(|_| StoreError::Invalid {
        field: "rules",
        problem: "unserialisable",
    })
}

fn check(draft: &CollectionDraft) -> Result<String> {
    let name = draft.name.trim().to_string();
    if name.is_empty() {
        return Err(StoreError::Invalid {
            field: "name",
            problem: "empty",
        });
    }
    Ok(name)
}

fn ids_json(ids: &[Uuid]) -> String {
    let mut seen: Vec<Uuid> = Vec::with_capacity(ids.len());
    for id in ids {
        if !seen.contains(id) {
            seen.push(*id);
        }
    }
    serde_json::to_string(&seen).unwrap_or_else(|_| "[]".into())
}

const COLUMNS: &str = "id, name, symbol, color, all_sources, source_ids, rules, notify, sort_index";

type Row = (String, CollectionRecord, String, String);

fn record(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok((
        row.get(0)?,
        CollectionRecord {
            id: Uuid::nil(),
            name: row.get(1)?,
            symbol: row.get(2)?,
            color: row.get(3)?,
            all_sources: row.get(4)?,
            source_ids: Vec::new(),
            rules: CollectionRules::default(),
            notify: row.get(7)?,
            sort_index: row.get(8)?,
        },
        row.get(5)?,
        row.get(6)?,
    ))
}

fn finish((id, mut collection, ids, rules): Row) -> Result<CollectionRecord> {
    collection.id = parse_uuid(&id)?;
    collection.source_ids = serde_json::from_str(&ids).unwrap_or_default();
    collection.rules = serde_json::from_str::<NewsRulesPayload>(&rules)
        .map(CollectionRules::from)
        .unwrap_or_default();
    Ok(collection)
}

impl Store {
    /// Make a collection; it sorts after every other one.
    pub fn add_collection(&self, draft: CollectionDraft) -> Result<CollectionRecord> {
        let name = check(&draft)?;
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let next: i64 = tx.query_row(
            "SELECT coalesce(max(sort_index) + 1, 0) FROM news_collections WHERE deleted = 0",
            [],
            |row| row.get(0),
        )?;
        let id = Uuid::now_v7();
        let clock = tick(&tx, self.device)?;
        tx.execute(
            "INSERT INTO news_collections
                (id, vault_id, name, symbol, color, all_sources, source_ids, rules,
                 notify, sort_index, hlc_wall_ms, hlc_counter, hlc_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                id.to_string(),
                vault_id(&tx)?,
                name,
                draft.symbol.trim(),
                draft.color.trim(),
                draft.all_sources,
                ids_json(&draft.source_ids),
                rules_json(draft.rules, None)?,
                draft.notify,
                next,
                clock.wall_ms as i64,
                clock.counter,
                clock.device,
            ],
        )?;
        let collection = collection_in(&tx, id)?.ok_or(StoreError::UnknownCollection(id))?;
        tx.commit()?;
        Ok(collection)
    }

    /// Replace a collection's settings; where it sorts stays.
    pub fn update_collection(&self, id: Uuid, draft: CollectionDraft) -> Result<CollectionRecord> {
        let name = check(&draft)?;
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let old_rules: Option<String> = tx
            .query_row(
                "SELECT rules FROM news_collections WHERE id = ?1 AND deleted = 0",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let old_rules = old_rules.ok_or(StoreError::UnknownCollection(id))?;
        let clock = tick(&tx, self.device)?;
        tx.execute(
            "UPDATE news_collections
                SET name = ?2, symbol = ?3, color = ?4, all_sources = ?5,
                    source_ids = ?6, rules = ?7, notify = ?8,
                    rev = rev + 1, dirty = 1,
                    hlc_wall_ms = ?9, hlc_counter = ?10, hlc_device = ?11
              WHERE id = ?1",
            params![
                id.to_string(),
                name,
                draft.symbol.trim(),
                draft.color.trim(),
                draft.all_sources,
                ids_json(&draft.source_ids),
                rules_json(draft.rules, Some(&old_rules))?,
                draft.notify,
                clock.wall_ms as i64,
                clock.counter,
                clock.device,
            ],
        )?;
        let collection = collection_in(&tx, id)?.ok_or(StoreError::UnknownCollection(id))?;
        tx.commit()?;
        Ok(collection)
    }

    pub fn delete_collection(&self, id: Uuid) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let clock = tick(&tx, self.device)?;
        let changed = tx.execute(
            "UPDATE news_collections
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
            return Err(StoreError::UnknownCollection(id));
        }
        tx.commit()?;
        Ok(())
    }

    /// Every collection, in sidebar order.
    pub fn list_collections(&self) -> Result<Vec<CollectionRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM news_collections WHERE deleted = 0
              ORDER BY sort_index, id"
        ))?;
        let rows = stmt
            .query_map([], record)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(finish).collect()
    }

    /// Put the collections in this order. Ids not listed keep their relative
    /// order after the listed ones; unknown ids are ignored. Only rows whose
    /// place changed are written.
    pub fn reorder_collections(&self, ids: &[Uuid]) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let current: Vec<(String, i64)> = {
            let mut stmt = tx.prepare(
                "SELECT id, sort_index FROM news_collections WHERE deleted = 0
                  ORDER BY sort_index, id",
            )?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        let wanted: Vec<String> = ids.iter().map(Uuid::to_string).collect();
        let mut order: Vec<&(String, i64)> = Vec::with_capacity(current.len());
        for id in &wanted {
            if let Some(row) = current.iter().find(|(c, _)| c == id) {
                if !order.iter().any(|(o, _)| o == id) {
                    order.push(row);
                }
            }
        }
        for row in &current {
            if !order.iter().any(|(o, _)| *o == row.0) {
                order.push(row);
            }
        }
        for (index, (id, old)) in order.into_iter().enumerate() {
            let index = index as i64;
            if *old == index {
                continue;
            }
            let clock = tick(&tx, self.device)?;
            tx.execute(
                "UPDATE news_collections
                    SET sort_index = ?2, rev = rev + 1, dirty = 1,
                        hlc_wall_ms = ?3, hlc_counter = ?4, hlc_device = ?5
                  WHERE id = ?1",
                params![id, index, clock.wall_ms as i64, clock.counter, clock.device],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

fn collection_in(tx: &Transaction, id: Uuid) -> Result<Option<CollectionRecord>> {
    tx.query_row(
        &format!("SELECT {COLUMNS} FROM news_collections WHERE id = ?1 AND deleted = 0"),
        [id.to_string()],
        record,
    )
    .optional()?
    .map(finish)
    .transpose()
}

/// Take a deleted source out of every collection that names it — a synced
/// write per collection changed.
pub(crate) fn forget_source(tx: &Transaction, device: u32, source: Uuid) -> Result<()> {
    let rows: Vec<(String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT id, source_ids FROM news_collections
              WHERE deleted = 0 AND instr(source_ids, ?1) > 0",
        )?;
        let rows = stmt
            .query_map([source.to_string()], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (id, ids) in rows {
        let mut list: Vec<Uuid> = serde_json::from_str(&ids).unwrap_or_default();
        let before = list.len();
        list.retain(|s| *s != source);
        if list.len() == before {
            continue;
        }
        let clock = tick(tx, device)?;
        tx.execute(
            "UPDATE news_collections
                SET source_ids = ?2, rev = rev + 1, dirty = 1,
                    hlc_wall_ms = ?3, hlc_counter = ?4, hlc_device = ?5
              WHERE id = ?1",
            params![
                id,
                ids_json(&list),
                clock.wall_ms as i64,
                clock.counter,
                clock.device
            ],
        )?;
    }
    Ok(())
}
