// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Database schema. Migrations are append-only: never edit a released
//! migration, add a new one instead. The schema version is tracked in
//! SQLite's `user_version`.

use rusqlite::Connection;

use crate::{Result, StoreError};

/// Version 1: sessions, laps and the blob index.
///
/// Conventions:
/// - ids are UUID strings, timestamps are UTC `YYYY-MM-DDTHH:MM:SS.mmmZ`
///   (fixed width, so they sort correctly as text)
/// - structured, rarely queried data is stored as JSON text
const V1_INITIAL: &str = r#"
CREATE TABLE sessions (
    id            TEXT PRIMARY KEY NOT NULL,
    game          TEXT NOT NULL,
    game_version  TEXT,
    track_key     TEXT NOT NULL,
    car_key       TEXT NOT NULL,
    session_type  TEXT NOT NULL,
    started_at    TEXT NOT NULL,
    ended_at      TEXT,
    conditions    TEXT NOT NULL DEFAULT '{}',
    setup         TEXT,
    session_info  TEXT,
    origin        TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
) STRICT;

CREATE INDEX sessions_by_start ON sessions (started_at DESC);
CREATE INDEX sessions_by_track_car ON sessions (game, track_key, car_key);

CREATE TABLE blobs (
    hash        TEXT PRIMARY KEY NOT NULL,
    size        INTEGER NOT NULL,
    created_at  TEXT NOT NULL
) STRICT;

CREATE TABLE laps (
    id               TEXT PRIMARY KEY NOT NULL,
    session_id       TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    number           INTEGER NOT NULL,
    lap_time_ms      INTEGER,
    sector_times_ms  TEXT NOT NULL DEFAULT '[]',
    valid            INTEGER NOT NULL CHECK (valid IN (0, 1)),
    invalid_reason   TEXT,
    flags            TEXT NOT NULL DEFAULT '{}',
    blob_hash        TEXT NOT NULL REFERENCES blobs (hash),
    sample_rate_hz   REAL NOT NULL,
    sample_count     INTEGER NOT NULL,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL
) STRICT;

CREATE INDEX laps_by_session ON laps (session_id, number);
CREATE INDEX laps_by_blob ON laps (blob_hash);
"#;

/// Version 2: raw spool files (docs/PLAN.md §5.1). `path` is relative to
/// the data directory.
const V2_RAW_SPOOL: &str = r#"
CREATE TABLE raw_spools (
    id          TEXT PRIMARY KEY NOT NULL,
    game        TEXT NOT NULL,
    path        TEXT NOT NULL UNIQUE,
    state       TEXT NOT NULL CHECK (state IN ('recording', 'processed', 'failed')),
    error       TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
) STRICT;

CREATE INDEX raw_spools_by_state ON raw_spools (state);
"#;

/// Version 3: human-readable track and car names, and key/value settings.
const V3_NAMES_AND_SETTINGS: &str = r#"
ALTER TABLE sessions ADD COLUMN track_name TEXT NOT NULL DEFAULT '';
ALTER TABLE sessions ADD COLUMN car_name TEXT NOT NULL DEFAULT '';

CREATE TABLE settings (
    key         TEXT PRIMARY KEY NOT NULL,
    value       TEXT NOT NULL,
    updated_at  TEXT NOT NULL
) STRICT;
"#;

/// Version 4: finding the same session/lap recorded twice (live and from a
/// game file). `start_time_s` is the game session time of the lap's first
/// sample; an alias maps a session id that turned out to be a duplicate to
/// the session that was stored first.
const V4_DUPLICATES: &str = r#"
ALTER TABLE laps ADD COLUMN start_time_s REAL;

CREATE TABLE session_aliases (
    alias_id    TEXT PRIMARY KEY NOT NULL,
    session_id  TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    created_at  TEXT NOT NULL
) STRICT;

CREATE INDEX session_aliases_by_session ON session_aliases (session_id);
"#;

/// Version 5: sync with a server (docs/PLAN.md §9.5). `sync_pushed`
/// remembers which version (`updated_at`) of a session or lap the server
/// has; `sync_deleted` lists deleted sessions the server must still hear
/// about.
const V5_SYNC: &str = r#"
CREATE TABLE sync_pushed (
    kind     TEXT NOT NULL CHECK (kind IN ('session', 'lap')),
    id       TEXT NOT NULL,
    version  TEXT NOT NULL,
    PRIMARY KEY (kind, id)
) STRICT;

CREATE TABLE sync_deleted (
    session_id  TEXT PRIMARY KEY NOT NULL,
    deleted_at  TEXT NOT NULL
) STRICT;
"#;

/// All migrations in order. Index + 1 = schema version.
const MIGRATIONS: &[&str] = &[
    V1_INITIAL,
    V2_RAW_SPOOL,
    V3_NAMES_AND_SETTINGS,
    V4_DUPLICATES,
    V5_SYNC,
];

/// Migrates the database to the latest schema version. Each migration runs
/// in its own transaction together with the `user_version` bump.
pub(crate) fn migrate(conn: &mut Connection) -> Result<()> {
    let current: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let latest = MIGRATIONS.len() as u32;
    if current > latest {
        return Err(StoreError::SchemaTooNew {
            found: current,
            supported: latest,
        });
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let version = index as u32 + 1;
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
        tracing::info!(version, "database migrated");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_fresh_database_and_is_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        migrate(&mut conn).unwrap();
        let v: u32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
    }

    #[test]
    fn refuses_newer_schema() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 999).unwrap();
        assert!(matches!(
            migrate(&mut conn),
            Err(StoreError::SchemaTooNew { found: 999, .. })
        ));
    }
}
