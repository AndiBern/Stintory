// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Row, params};
use rz_model::api::SessionSummary;
use rz_model::{Lap, LapId, LapTelemetry, NewLap, Session, SessionId};

use crate::blobs::BlobStore;
use crate::migrations;
use crate::{Result, StoreError, timefmt};

const DB_FILE: &str = "rundenzeit.db";
const BLOB_DIR: &str = "blobs";

/// The local store: SQLite database + blob directory.
pub struct Store {
    conn: Connection,
    blobs: BlobStore,
    root: PathBuf,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Store {
    /// Opens the store in `dir` (created if missing) and migrates the
    /// database to the latest schema.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let root = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        let mut conn = Connection::open(root.join(DB_FILE))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrations::migrate(&mut conn)?;
        let blobs = BlobStore::open(root.join(BLOB_DIR))?;
        tracing::debug!(root = %root.display(), "store opened");
        Ok(Self { conn, blobs, root })
    }

    /// Data directory of this store.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The blob directory.
    pub fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Inserts a new session.
    pub fn insert_session(&mut self, s: &Session) -> Result<()> {
        let now = timefmt::now();
        self.conn.execute(
            "INSERT INTO sessions (id, game, game_version, track_key, car_key, session_type,
                started_at, ended_at, conditions, setup, session_info, origin, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
            params![
                s.id.to_string(),
                s.game.as_str(),
                s.game_version,
                s.track_key,
                s.car_key,
                s.session_type.as_str(),
                timefmt::encode(s.started_at),
                s.ended_at.map(timefmt::encode),
                serde_json::to_string(&s.conditions)?,
                s.setup.as_ref().map(serde_json::to_string).transpose()?,
                s.session_info.as_ref().map(serde_json::to_string).transpose()?,
                serde_json::to_string(&s.origin)?,
                now,
            ],
        )?;
        Ok(())
    }

    /// Inserts the session, or updates it if a session with this id exists
    /// (re-import, or a session split across several files).
    pub fn upsert_session(&mut self, s: &Session) -> Result<()> {
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM sessions WHERE id = ?1)",
            [s.id.to_string()],
            |r| r.get(0),
        )?;
        if exists {
            self.update_session(s)
        } else {
            self.insert_session(s)
        }
    }

    /// Updates a session's mutable fields (everything except id, game,
    /// start time and origin), e.g. when the game reports new session info
    /// or the session ended.
    pub fn update_session(&mut self, s: &Session) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE sessions SET game_version = ?2, track_key = ?3, car_key = ?4,
                session_type = ?5, ended_at = ?6, conditions = ?7, setup = ?8,
                session_info = ?9, updated_at = ?10
             WHERE id = ?1",
            params![
                s.id.to_string(),
                s.game_version,
                s.track_key,
                s.car_key,
                s.session_type.as_str(),
                s.ended_at.map(timefmt::encode),
                serde_json::to_string(&s.conditions)?,
                s.setup.as_ref().map(serde_json::to_string).transpose()?,
                s.session_info
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                timefmt::now(),
            ],
        )?;
        if changed == 0 {
            return Err(StoreError::SessionNotFound(s.id));
        }
        Ok(())
    }

    /// Deletes a session with all its laps. Blobs no longer referenced by any
    /// lap are removed from disk.
    pub fn delete_session(&mut self, id: SessionId) -> Result<()> {
        let tx = self.conn.transaction()?;
        let deleted = tx.execute("DELETE FROM sessions WHERE id = ?1", [id.to_string()])?;
        if deleted == 0 {
            return Err(StoreError::SessionNotFound(id));
        }
        let orphans: Vec<String> = {
            let mut stmt = tx
                .prepare("SELECT hash FROM blobs WHERE hash NOT IN (SELECT blob_hash FROM laps)")?;
            stmt.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        tx.execute(
            "DELETE FROM blobs WHERE hash NOT IN (SELECT blob_hash FROM laps)",
            [],
        )?;
        tx.commit()?;
        for hash in orphans {
            if let Ok(hash) = hash.parse() {
                self.blobs.remove(&hash)?;
            }
        }
        Ok(())
    }

    /// Loads a session.
    pub fn session(&self, id: SessionId) -> Result<Session> {
        self.conn
            .query_row(
                "SELECT id, game, game_version, track_key, car_key, session_type, started_at,
                        ended_at, conditions, setup, session_info, origin
                 FROM sessions WHERE id = ?1",
                [id.to_string()],
                |row| Ok(read_session(row)),
            )
            .optional()?
            .ok_or(StoreError::SessionNotFound(id))?
    }

    /// All sessions, newest first, with lap count and best valid lap time.
    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.game, s.track_key, s.car_key, s.session_type, s.started_at,
                    COUNT(l.id),
                    MIN(CASE WHEN l.valid = 1
                              AND COALESCE(json_extract(l.flags, '$.out_lap'), 0) = 0
                              AND COALESCE(json_extract(l.flags, '$.in_lap'), 0) = 0
                              AND COALESCE(json_extract(l.flags, '$.incomplete'), 0) = 0
                             THEN l.lap_time_ms END)
             FROM sessions s LEFT JOIN laps l ON l.session_id = s.id
             GROUP BY s.id
             ORDER BY s.started_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((|| {
                Ok(SessionSummary {
                    id: parse(row, 0, "sessions.id")?,
                    game: parse(row, 1, "sessions.game")?,
                    track_key: row.get(2)?,
                    car_key: row.get(3)?,
                    session_type: parse(row, 4, "sessions.session_type")?,
                    started_at: timefmt::decode("sessions.started_at", &row.get::<_, String>(5)?)?,
                    lap_count: row.get(6)?,
                    best_lap_ms: row.get(7)?,
                })
            })())
        })?;
        rows.map(|r| r?).collect()
    }

    /// Stores a lap: writes the telemetry blob, then inserts the lap row in
    /// one transaction. Returns the stored lap. If a lap with this id already
    /// exists, nothing is written and the existing lap is returned.
    pub fn insert_lap(&mut self, new: NewLap, telemetry: &LapTelemetry) -> Result<Lap> {
        // Idempotent: lap ids are derived, so a re-import yields the same id.
        match self.lap(new.id) {
            Ok(existing) => return Ok(existing),
            Err(StoreError::LapNotFound(_)) => {}
            Err(e) => return Err(e),
        }
        let (bytes, _) = telemetry.to_blob()?;
        let hash = self.blobs.put(&bytes)?;
        let sample_count = u32::try_from(telemetry.len()).map_err(|_| StoreError::Corrupt {
            column: "laps.sample_count",
            message: "too many samples".into(),
        })?;
        let lap = new.into_lap(hash, telemetry.sample_rate_hz(), sample_count);

        let now = timefmt::now();
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO blobs (hash, size, created_at) VALUES (?1, ?2, ?3)",
            params![lap.blob_hash.as_str(), bytes.len() as i64, now],
        )?;
        let inserted = tx.execute(
            "INSERT INTO laps (id, session_id, number, lap_time_ms, sector_times_ms, valid,
                invalid_reason, flags, blob_hash, sample_rate_hz, sample_count, created_at, updated_at)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12
             WHERE EXISTS (SELECT 1 FROM sessions WHERE id = ?2)",
            params![
                lap.id.to_string(),
                lap.session_id.to_string(),
                lap.number,
                lap.lap_time_ms,
                serde_json::to_string(&lap.sector_times_ms)?,
                lap.valid,
                lap.invalid_reason,
                serde_json::to_string(&lap.flags)?,
                lap.blob_hash.as_str(),
                lap.sample_rate_hz,
                lap.sample_count,
                now,
            ],
        )?;
        if inserted == 0 {
            return Err(StoreError::SessionNotFound(lap.session_id));
        }
        tx.commit()?;
        Ok(lap)
    }

    /// All laps of a session, ordered by lap number.
    pub fn laps(&self, session_id: SessionId) -> Result<Vec<Lap>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, number, lap_time_ms, sector_times_ms, valid, invalid_reason,
                    flags, blob_hash, sample_rate_hz, sample_count
             FROM laps WHERE session_id = ?1 ORDER BY number, id",
        )?;
        let rows = stmt.query_map([session_id.to_string()], |row| Ok(read_lap(row)))?;
        rows.map(|r| r?).collect()
    }

    /// Loads one lap's metadata.
    pub fn lap(&self, id: LapId) -> Result<Lap> {
        self.conn
            .query_row(
                "SELECT id, session_id, number, lap_time_ms, sector_times_ms, valid, invalid_reason,
                        flags, blob_hash, sample_rate_hz, sample_count
                 FROM laps WHERE id = ?1",
                [id.to_string()],
                |row| Ok(read_lap(row)),
            )
            .optional()?
            .ok_or(StoreError::LapNotFound(id))?
    }

    /// Loads one lap's telemetry from its blob.
    pub fn lap_telemetry(&self, id: LapId) -> Result<LapTelemetry> {
        let lap = self.lap(id)?;
        let bytes = self.blobs.get(&lap.blob_hash)?;
        Ok(LapTelemetry::from_parquet(bytes)?)
    }
}

/// Reads column `idx` as text and parses it with `FromStr`.
fn parse<T>(row: &Row<'_>, idx: usize, column: &'static str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let s: String = row.get(idx)?;
    s.parse().map_err(|e: T::Err| StoreError::Corrupt {
        column,
        message: format!("{s:?}: {e}"),
    })
}

fn json<T: serde::de::DeserializeOwned>(row: &Row<'_>, idx: usize) -> Result<Option<T>> {
    let s: Option<String> = row.get(idx)?;
    Ok(s.map(|s| serde_json::from_str(&s)).transpose()?)
}

fn read_session(row: &Row<'_>) -> Result<Session> {
    Ok(Session {
        id: parse(row, 0, "sessions.id")?,
        game: parse(row, 1, "sessions.game")?,
        game_version: row.get(2)?,
        track_key: row.get(3)?,
        car_key: row.get(4)?,
        session_type: parse(row, 5, "sessions.session_type")?,
        started_at: timefmt::decode("sessions.started_at", &row.get::<_, String>(6)?)?,
        ended_at: row
            .get::<_, Option<String>>(7)?
            .map(|s| timefmt::decode("sessions.ended_at", &s))
            .transpose()?,
        conditions: json(row, 8)?.unwrap_or_default(),
        setup: json(row, 9)?,
        session_info: json(row, 10)?,
        origin: json(row, 11)?.ok_or(StoreError::Corrupt {
            column: "sessions.origin",
            message: "NULL".into(),
        })?,
    })
}

fn read_lap(row: &Row<'_>) -> Result<Lap> {
    Ok(Lap {
        id: parse(row, 0, "laps.id")?,
        session_id: parse(row, 1, "laps.session_id")?,
        number: row.get(2)?,
        lap_time_ms: row.get(3)?,
        sector_times_ms: json(row, 4)?.unwrap_or_default(),
        valid: row.get(5)?,
        invalid_reason: row.get(6)?,
        flags: json(row, 7)?.unwrap_or_default(),
        blob_hash: parse(row, 8, "laps.blob_hash")?,
        sample_rate_hz: row.get(9)?,
        sample_count: row.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rz_model::channel::names;
    use rz_model::session::Conditions;
    use rz_model::{Channel, ChannelData, GameId, LapFlags, Origin, SessionType, Unit};
    use time::macros::datetime;

    fn session(started: time::OffsetDateTime) -> Session {
        Session {
            id: SessionId::new(),
            game: GameId::Iracing,
            game_version: Some("2026.11".into()),
            track_key: "iracing:261:Grand Prix".into(),
            car_key: "iracing:132".into(),
            session_type: SessionType::Practice,
            started_at: started,
            ended_at: None,
            conditions: Conditions {
                air_temp_c: Some(21.5),
                track_temp_c: Some(30.0),
                weather: None,
            },
            setup: Some(serde_json::json!({"brake_bias": 0.54})),
            session_info: None,
            origin: Origin::Own,
        }
    }

    fn telemetry(samples: usize, speed: f32) -> LapTelemetry {
        LapTelemetry::new(60.0)
            .with_channel(Channel::new(
                names::SPEED,
                Unit::MeterPerSecond,
                ChannelData::F32(vec![speed; samples]),
            ))
            .unwrap()
    }

    fn new_lap(session_id: SessionId, number: u32, time_ms: u32, valid: bool) -> NewLap {
        NewLap {
            id: LapId::new(),
            session_id,
            number,
            lap_time_ms: Some(time_ms),
            sector_times_ms: vec![30_000, 30_000, time_ms - 60_000],
            valid,
            invalid_reason: (!valid).then(|| "off track".into()),
            flags: LapFlags::default(),
        }
    }

    #[test]
    fn session_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let s = session(datetime!(2026-11-10 18:00 UTC));
        store.insert_session(&s).unwrap();
        assert_eq!(store.session(s.id).unwrap(), s);
        assert!(matches!(
            store.session(SessionId::new()),
            Err(StoreError::SessionNotFound(_))
        ));
    }

    #[test]
    fn laps_and_telemetry_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let s = session(datetime!(2026-11-10 18:00 UTC));
        store.insert_session(&s).unwrap();

        let t = telemetry(5400, 42.0);
        let lap = store
            .insert_lap(new_lap(s.id, 1, 92_345, true), &t)
            .unwrap();
        assert_eq!(lap.sample_count, 5400);
        assert_eq!(store.laps(s.id).unwrap(), vec![lap.clone()]);
        assert_eq!(store.lap_telemetry(lap.id).unwrap(), t);
    }

    #[test]
    fn identical_telemetry_is_stored_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let s = session(datetime!(2026-11-10 18:00 UTC));
        store.insert_session(&s).unwrap();
        let t = telemetry(100, 10.0);
        let a = store
            .insert_lap(new_lap(s.id, 1, 90_000, true), &t)
            .unwrap();
        let b = store
            .insert_lap(new_lap(s.id, 2, 91_000, true), &t)
            .unwrap();
        assert_eq!(a.blob_hash, b.blob_hash);
        let blob_rows: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(blob_rows, 1);
    }

    #[test]
    fn reinserting_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let mut s = session(datetime!(2026-11-10 18:00 UTC));
        store.upsert_session(&s).unwrap();
        s.ended_at = Some(datetime!(2026-11-10 18:30 UTC));
        store.upsert_session(&s).unwrap();
        assert_eq!(store.session(s.id).unwrap().ended_at, s.ended_at);

        let new = new_lap(s.id, 1, 90_000, true);
        let a = store.insert_lap(new.clone(), &telemetry(10, 1.0)).unwrap();
        let b = store.insert_lap(new, &telemetry(10, 1.0)).unwrap();
        assert_eq!(a, b);
        assert_eq!(store.laps(s.id).unwrap().len(), 1);
    }

    #[test]
    fn lap_for_unknown_session_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let err = store
            .insert_lap(
                new_lap(SessionId::new(), 1, 90_000, true),
                &telemetry(10, 1.0),
            )
            .unwrap_err();
        assert!(matches!(err, StoreError::SessionNotFound(_)));
    }

    #[test]
    fn list_sessions_summarises_laps() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let older = session(datetime!(2026-11-09 18:00 UTC));
        let newer = session(datetime!(2026-11-10 18:00 UTC));
        store.insert_session(&older).unwrap();
        store.insert_session(&newer).unwrap();
        store
            .insert_lap(new_lap(newer.id, 1, 95_000, true), &telemetry(10, 1.0))
            .unwrap();
        store
            .insert_lap(new_lap(newer.id, 2, 89_000, false), &telemetry(10, 2.0))
            .unwrap();
        store
            .insert_lap(new_lap(newer.id, 3, 92_000, true), &telemetry(10, 3.0))
            .unwrap();

        let list = store.list_sessions().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, newer.id, "newest first");
        assert_eq!(list[0].lap_count, 3);
        assert_eq!(
            list[0].best_lap_ms,
            Some(92_000),
            "invalid laps don't count"
        );
        assert_eq!(list[1].lap_count, 0);
        assert_eq!(list[1].best_lap_ms, None);
    }

    #[test]
    fn out_laps_do_not_count_as_best() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let s = session(datetime!(2026-11-10 18:00 UTC));
        store.insert_session(&s).unwrap();
        let mut out_lap = new_lap(s.id, 1, 70_000, true);
        out_lap.flags.out_lap = true;
        store.insert_lap(out_lap, &telemetry(10, 1.0)).unwrap();
        store
            .insert_lap(new_lap(s.id, 2, 92_000, true), &telemetry(10, 2.0))
            .unwrap();
        assert_eq!(store.list_sessions().unwrap()[0].best_lap_ms, Some(92_000));
    }

    #[test]
    fn update_session_changes_mutable_fields() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let mut s = session(datetime!(2026-11-10 18:00 UTC));
        store.insert_session(&s).unwrap();
        s.ended_at = Some(datetime!(2026-11-10 18:45 UTC));
        s.conditions.weather = Some("cloudy".into());
        store.update_session(&s).unwrap();
        assert_eq!(store.session(s.id).unwrap(), s);
    }

    #[test]
    fn delete_session_removes_laps_and_unused_blobs() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let a = session(datetime!(2026-11-10 18:00 UTC));
        let b = session(datetime!(2026-11-10 19:00 UTC));
        store.insert_session(&a).unwrap();
        store.insert_session(&b).unwrap();
        let shared = telemetry(10, 1.0);
        let only_a = telemetry(10, 2.0);
        let lap_shared = store
            .insert_lap(new_lap(a.id, 1, 90_000, true), &shared)
            .unwrap();
        let lap_only_a = store
            .insert_lap(new_lap(a.id, 2, 90_000, true), &only_a)
            .unwrap();
        store
            .insert_lap(new_lap(b.id, 1, 90_000, true), &shared)
            .unwrap();

        store.delete_session(a.id).unwrap();
        assert!(store.laps(a.id).unwrap().is_empty());
        assert!(
            store.blobs().contains(&lap_shared.blob_hash),
            "still used by b"
        );
        assert!(!store.blobs().contains(&lap_only_a.blob_hash));
    }

    #[test]
    fn reopening_keeps_data() {
        let dir = tempfile::tempdir().unwrap();
        let s = session(datetime!(2026-11-10 18:00 UTC));
        {
            let mut store = Store::open(dir.path()).unwrap();
            store.insert_session(&s).unwrap();
        }
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.session(s.id).unwrap(), s);
    }
}
