//! The schema, tracked in SQLite's own `user_version`.
//!
//! Each version is one SQL batch applied in one transaction, so an interrupted
//! upgrade leaves the previous schema intact rather than half of the new one.
//! There is one version so far.

use crate::{Result, StoreError};
use rusqlite::{params, Connection};
use uuid::Uuid;

pub const SCHEMA_VERSION: i64 = 1;

/// Synced tables carry the sync header — id, vault, hybrid logical clock,
/// revision, tombstone, the outbox flag, the server's version and the fields a
/// newer build wrote that this one does not know.
const V1: &str = r#"
CREATE TABLE meta (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    -- The device part of every clock this install writes.
    device_id       INTEGER NOT NULL,
    vault_id        TEXT    NOT NULL,
    clock_wall_ms   INTEGER NOT NULL DEFAULT 0,
    clock_counter   INTEGER NOT NULL DEFAULT 0,
    -- Whose labels the vault key in the secret store goes with (`ssh` or
    -- `rdp`); NULL while this device holds no vault key.
    vault_flavor    TEXT
);

CREATE TABLE news_sources (
    id              TEXT    PRIMARY KEY,
    vault_id        TEXT    NOT NULL,
    url             TEXT    NOT NULL,
    title           TEXT,
    site_url        TEXT,
    notify          INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL DEFAULT 0,
    hlc_wall_ms     INTEGER NOT NULL,
    hlc_counter     INTEGER NOT NULL,
    hlc_device      INTEGER NOT NULL,
    rev             INTEGER NOT NULL DEFAULT 1,
    deleted         INTEGER NOT NULL DEFAULT 0,
    dirty           INTEGER NOT NULL DEFAULT 1,
    server_seq      INTEGER NOT NULL DEFAULT 0,
    sync_extra      TEXT
);
CREATE INDEX news_sources_dirty ON news_sources (hlc_wall_ms) WHERE dirty = 1;

CREATE TABLE news_collections (
    id              TEXT    PRIMARY KEY,
    vault_id        TEXT    NOT NULL,
    name            TEXT    NOT NULL,
    symbol          TEXT    NOT NULL DEFAULT '',
    color           TEXT    NOT NULL DEFAULT '',
    all_sources     INTEGER NOT NULL DEFAULT 0,
    -- JSON: a list of source ids.
    source_ids      TEXT    NOT NULL DEFAULT '[]',
    -- JSON: `NewsRulesPayload`, its unknown fields included.
    rules           TEXT    NOT NULL DEFAULT '{}',
    notify          INTEGER NOT NULL DEFAULT 0,
    sort_index      INTEGER NOT NULL DEFAULT 0,
    hlc_wall_ms     INTEGER NOT NULL,
    hlc_counter     INTEGER NOT NULL,
    hlc_device      INTEGER NOT NULL,
    rev             INTEGER NOT NULL DEFAULT 1,
    deleted         INTEGER NOT NULL DEFAULT 0,
    dirty           INTEGER NOT NULL DEFAULT 1,
    server_seq      INTEGER NOT NULL DEFAULT 0,
    sync_extra      TEXT
);
CREATE INDEX news_collections_dirty ON news_collections (hlc_wall_ms) WHERE dirty = 1;

CREATE TABLE news_marks (
    id              TEXT    PRIMARY KEY,
    vault_id        TEXT    NOT NULL,
    article         TEXT    NOT NULL,
    source_id       TEXT    NOT NULL,
    read            INTEGER NOT NULL DEFAULT 0,
    starred         INTEGER NOT NULL DEFAULT 0,
    published_at    INTEGER,
    hlc_wall_ms     INTEGER NOT NULL,
    hlc_counter     INTEGER NOT NULL,
    hlc_device      INTEGER NOT NULL,
    rev             INTEGER NOT NULL DEFAULT 1,
    deleted         INTEGER NOT NULL DEFAULT 0,
    dirty           INTEGER NOT NULL DEFAULT 1,
    server_seq      INTEGER NOT NULL DEFAULT 0,
    sync_extra      TEXT
);
CREATE INDEX news_marks_article ON news_marks (article);
CREATE INDEX news_marks_dirty ON news_marks (hlc_wall_ms) WHERE dirty = 1;

-- Local only: the fetched articles, this device's cache of its feeds.
CREATE TABLE articles (
    key             TEXT    PRIMARY KEY,
    source_id       TEXT    NOT NULL,
    guid            TEXT    NOT NULL,
    title           TEXT    NOT NULL,
    link            TEXT,
    summary         TEXT    NOT NULL DEFAULT '',
    content_html    TEXT    NOT NULL DEFAULT '',
    author          TEXT,
    image_url       TEXT,
    published_at    INTEGER,
    fetched_at      INTEGER NOT NULL
);
CREATE INDEX articles_by_source ON articles (source_id, published_at);

-- Local only: articles pruned here, so a feed that still lists one does not
-- bring it back as new.
CREATE TABLE pruned_articles (
    key             TEXT    PRIMARY KEY,
    source_id       TEXT    NOT NULL,
    pruned_at       INTEGER NOT NULL
);

-- Local only: what the last fetch of a source said.
CREATE TABLE fetch_state (
    source_id       TEXT    PRIMARY KEY,
    etag            TEXT,
    last_modified   TEXT,
    last_attempt    INTEGER,
    last_success    INTEGER,
    last_error      TEXT
);

-- Local only: articles a notification went out for.
CREATE TABLE notified (
    key             TEXT    PRIMARY KEY
);

CREATE TABLE manifests (
    id              TEXT    PRIMARY KEY,
    vault_id        TEXT    NOT NULL,
    entries         BLOB    NOT NULL,
    hlc_wall_ms     INTEGER NOT NULL,
    hlc_counter     INTEGER NOT NULL,
    hlc_device      INTEGER NOT NULL,
    rev             INTEGER NOT NULL DEFAULT 1,
    deleted         INTEGER NOT NULL DEFAULT 0,
    dirty           INTEGER NOT NULL DEFAULT 1,
    server_seq      INTEGER NOT NULL DEFAULT 0
);

-- Versions of records given up here without a tombstone: marks pruned
-- locally. A manifest that lists one of these versions, or an older one, is
-- satisfied, and a pull that brings it back again does not reinsert it.
CREATE TABLE superseded (
    id              TEXT    PRIMARY KEY,
    kind            INTEGER NOT NULL,
    hlc_wall_ms     INTEGER NOT NULL,
    hlc_counter     INTEGER NOT NULL,
    hlc_device      INTEGER NOT NULL
);

CREATE TABLE manifest_violations (
    kind            INTEGER NOT NULL,
    id              TEXT    NOT NULL,
    problem         TEXT    NOT NULL,
    PRIMARY KEY (kind, id)
);

-- Where this device stands with a UwUSync server. The device key and the
-- account key are in the secret store, not here.
CREATE TABLE sync_state (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    server_url          TEXT,
    account_id          TEXT,
    device_id           TEXT,
    tls_fingerprint     TEXT,
    cursor              INTEGER NOT NULL DEFAULT 0,
    last_sync_ms        INTEGER,
    paired_ms           INTEGER,
    floor_manifest_id   TEXT,
    floor_wall_ms       INTEGER,
    floor_counter       INTEGER,
    floor_device        INTEGER
);

-- The vault header of a UwUSync account: not secret, and useless without the
-- master password and the account key. None in UwULock mode.
CREATE TABLE vault (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    vault_id            TEXT    NOT NULL,
    kdf_memory_kib      INTEGER NOT NULL,
    kdf_time_cost       INTEGER NOT NULL,
    kdf_parallelism     INTEGER NOT NULL,
    salt                BLOB    NOT NULL,
    wrapped_nonce       BLOB    NOT NULL,
    wrapped_blob        BLOB    NOT NULL,
    needs_account_key   INTEGER NOT NULL DEFAULT 0,
    flavor              TEXT    NOT NULL DEFAULT 'ssh'
);

-- Where this device stands with a UwULock server. Its tokens are in the
-- secret store.
CREATE TABLE lock_state (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    active              INTEGER NOT NULL DEFAULT 0,
    server_url          TEXT,
    email               TEXT,
    space_id            TEXT,
    signed_in_ms        INTEGER,
    device_identifier   TEXT    NOT NULL
);

CREATE TABLE lock_accounts (
    server_url          TEXT    NOT NULL,
    email               TEXT    NOT NULL,
    kdf                 TEXT,
    space_id            TEXT,
    left_spaces         TEXT    NOT NULL DEFAULT '[]',
    PRIMARY KEY (server_url, email)
);
"#;

pub fn migrate(conn: &mut Connection) -> Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        // An older build writing into a newer schema can corrupt data it does
        // not understand.
        return Err(StoreError::SchemaTooNew {
            found: version,
            known: SCHEMA_VERSION,
        });
    }
    if version < 1 {
        let tx = conn.transaction()?;
        tx.execute_batch(V1)?;
        tx.execute(
            "INSERT INTO meta (id, device_id, vault_id) VALUES (1, ?1, ?2)",
            params![i64::from(rand::random::<u32>()), Uuid::now_v7().to_string()],
        )?;
        tx.execute("INSERT INTO sync_state (id) VALUES (1)", [])?;
        tx.execute(
            "INSERT INTO lock_state (id, device_identifier) VALUES (1, ?1)",
            [Uuid::new_v4().to_string()],
        )?;
        tx.pragma_update(None, "user_version", 1)?;
        tx.commit()?;
        tracing::info!("store created at schema 1");
    }
    Ok(())
}
