// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Local side of the sync with a server (docs/PLAN.md §9.5): what needs to
//! be pushed, and applying what was pulled.
//!
//! A session or lap needs a push when its `updated_at` differs from the
//! version recorded in `sync_pushed`. Applying a pulled object records its
//! new local version as pushed, so it doesn't bounce back.

use rusqlite::{OptionalExtension, params};
use sy_model::{BlobHash, Lap, LapId, LapTelemetry, NewLap, Session, SessionId};

use crate::store::{read_lap, read_session};
use crate::{Result, Store, StoreError, timefmt};

/// What the server doesn't have yet.
#[derive(Debug, Clone, Default)]
pub struct PendingPush {
    /// New or changed sessions
    pub sessions: Vec<Session>,
    /// New or changed laps
    pub laps: Vec<Lap>,
    /// Deleted sessions the server still has
    pub deleted: Vec<SessionId>,
    versions: Vec<(&'static str, String, String)>,
}

impl PendingPush {
    /// Nothing to push.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty() && self.laps.is_empty() && self.deleted.is_empty()
    }
}

impl Store {
    /// Up to `limit` laps (plus all changed sessions and deletions) that the
    /// server doesn't have in their current version. Laps of other people
    /// (shared with us) are never pushed.
    pub fn pending_push(&self, limit: usize) -> Result<PendingPush> {
        let mut out = PendingPush::default();
        let own = "json_extract(s.origin, '$.kind') != 'synced'";
        let mut stmt = self.conn().prepare(&format!(
            "SELECT s.id, s.game, s.game_version, s.track_key, s.car_key, s.session_type,
                    s.started_at, s.ended_at, s.conditions, s.setup, s.session_info, s.origin,
                    s.track_name, s.car_name, s.updated_at
             FROM sessions s
             LEFT JOIN sync_pushed p ON p.kind = 'session' AND p.id = s.id
             WHERE (p.version IS NULL OR p.version != s.updated_at) AND {own}
             ORDER BY s.started_at"
        ))?;
        let rows = stmt.query_map([], |r| Ok((read_session(r), r.get::<_, String>(14)?)))?;
        for row in rows {
            let (session, version) = row?;
            let session = session?;
            out.versions
                .push(("session", session.id.to_string(), version));
            out.sessions.push(session);
        }
        let mut stmt = self.conn().prepare(&format!(
            "SELECT l.id, l.session_id, l.number, l.lap_time_ms, l.sector_times_ms, l.valid,
                    l.invalid_reason, l.flags, l.blob_hash, l.sample_rate_hz, l.sample_count,
                    l.start_time_s, l.updated_at
             FROM laps l JOIN sessions s ON s.id = l.session_id
             LEFT JOIN sync_pushed p ON p.kind = 'lap' AND p.id = l.id
             WHERE (p.version IS NULL OR p.version != l.updated_at) AND {own}
             ORDER BY s.started_at, l.number
             LIMIT ?1"
        ))?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok((read_lap(r), r.get::<_, String>(12)?))
        })?;
        for row in rows {
            let (lap, version) = row?;
            let lap = lap?;
            out.versions.push(("lap", lap.id.to_string(), version));
            out.laps.push(lap);
        }
        let mut stmt = self
            .conn()
            .prepare("SELECT session_id FROM sync_deleted ORDER BY deleted_at")?;
        for id in stmt.query_map([], |r| r.get::<_, String>(0))? {
            let id = id?;
            out.deleted
                .push(id.parse().map_err(|_| StoreError::Corrupt {
                    column: "sync_deleted.session_id",
                    message: id,
                })?);
        }
        Ok(out)
    }

    /// Records that the server has `pushed` (in the versions read by
    /// [`pending_push`](Self::pending_push)).
    pub fn mark_pushed(&mut self, pushed: &PendingPush) -> Result<()> {
        let tx = self.conn.transaction()?;
        for (kind, id, version) in &pushed.versions {
            tx.execute(
                "INSERT OR REPLACE INTO sync_pushed (kind, id, version) VALUES (?1, ?2, ?3)",
                params![kind, id, version],
            )?;
        }
        for id in &pushed.deleted {
            tx.execute(
                "DELETE FROM sync_deleted WHERE session_id = ?1",
                [id.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Forgets everything about the server (connecting to another one).
    pub fn reset_sync(&mut self) -> Result<()> {
        self.conn
            .execute_batch("DELETE FROM sync_pushed; DELETE FROM sync_deleted;")?;
        Ok(())
    }

    fn mark_current(&self, kind: &str, id: &str) -> Result<()> {
        let table = if kind == "session" {
            "sessions"
        } else {
            "laps"
        };
        self.conn.execute(
            &format!(
                "INSERT OR REPLACE INTO sync_pushed (kind, id, version)
                 SELECT ?1, id, updated_at FROM {table} WHERE id = ?2"
            ),
            params![kind, id],
        )?;
        Ok(())
    }

    /// Stores a session pulled from the server (or updates it). Returns the
    /// id it is stored under (see [`upsert_session`](Self::upsert_session)).
    pub fn apply_remote_session(&mut self, s: &Session) -> Result<SessionId> {
        let id = self.upsert_session(s)?;
        self.mark_current("session", &id.to_string())?;
        Ok(id)
    }

    /// True if a pulled lap needs its telemetry downloaded: the lap is new
    /// or has other telemetry, and the file isn't here.
    pub fn needs_telemetry(&self, lap: &Lap) -> Result<bool> {
        let local: Option<String> = self
            .conn
            .query_row(
                "SELECT blob_hash FROM laps WHERE id = ?1",
                [lap.id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        let same = local.as_deref() == Some(lap.blob_hash.as_str());
        Ok(!same && !self.blobs().contains(&lap.blob_hash))
    }

    /// Stores a lap pulled from the server, or updates the local copy.
    /// `telemetry` is the downloaded file, if [`needs_telemetry`] said so.
    /// A lap whose session isn't here, or that duplicates a local lap
    /// under another id, is skipped (returns false).
    ///
    /// [`needs_telemetry`]: Self::needs_telemetry
    pub fn apply_remote_lap(&mut self, lap: &Lap, telemetry: Option<&[u8]>) -> Result<bool> {
        let mut lap = lap.clone();
        lap.session_id = self.canonical_session_id(lap.session_id)?;
        if !self.session_exists(lap.session_id)? {
            return Ok(false);
        }
        if let Some(bytes) = telemetry {
            if BlobHash::of(bytes) != lap.blob_hash {
                return Err(StoreError::BlobCorrupt(lap.blob_hash.clone()));
            }
            LapTelemetry::from_parquet(bytes.to_vec())?;
            self.blobs().put(bytes)?;
        }
        if !self.blobs().contains(&lap.blob_hash) {
            return Err(StoreError::BlobMissing(lap.blob_hash.clone()));
        }
        let size = std::fs::metadata(self.blobs().path(&lap.blob_hash))?.len() as usize;

        match self.lap(lap.id) {
            Err(StoreError::LapNotFound(_)) => {
                let probe = NewLap {
                    id: lap.id,
                    session_id: lap.session_id,
                    number: lap.number,
                    start_time_s: lap.start_time_s,
                    lap_time_ms: lap.lap_time_ms,
                    sector_times_ms: Vec::new(),
                    valid: lap.valid,
                    invalid_reason: None,
                    flags: lap.flags,
                };
                if self.find_duplicate_lap(&probe)?.is_some() {
                    return Ok(false);
                }
                self.insert_lap_row(&lap, size)?;
            }
            Err(e) => return Err(e),
            Ok(old) => {
                if old == lap {
                    self.mark_current("lap", &lap.id.to_string())?;
                    return Ok(false);
                }
                self.update_lap_row(&lap, size)?;
                if old.blob_hash != lap.blob_hash {
                    self.remove_blob_if_unused(&old.blob_hash)?;
                }
            }
        }
        self.mark_current("lap", &lap.id.to_string())?;
        Ok(true)
    }

    /// Deletes a session the server says was deleted (without telling the
    /// server again). Unknown sessions are ignored.
    pub fn apply_remote_deletion(&mut self, id: SessionId) -> Result<()> {
        let id = self.canonical_session_id(id)?;
        match self.delete_session_inner(id, false) {
            Ok(()) | Err(StoreError::SessionNotFound(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn update_lap_row(&mut self, lap: &Lap, blob_size: usize) -> Result<()> {
        let now = timefmt::now();
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO blobs (hash, size, created_at) VALUES (?1, ?2, ?3)",
            params![lap.blob_hash.as_str(), blob_size as i64, now],
        )?;
        tx.execute(
            "UPDATE laps SET number = ?2, lap_time_ms = ?3, sector_times_ms = ?4, valid = ?5,
                invalid_reason = ?6, flags = ?7, blob_hash = ?8, sample_rate_hz = ?9,
                sample_count = ?10, start_time_s = ?11, updated_at = ?12
             WHERE id = ?1",
            params![
                lap.id.to_string(),
                lap.number,
                lap.lap_time_ms,
                serde_json::to_string(&lap.sector_times_ms)?,
                lap.valid,
                lap.invalid_reason,
                serde_json::to_string(&lap.flags)?,
                lap.blob_hash.as_str(),
                lap.sample_rate_hz,
                lap.sample_count,
                lap.start_time_s,
                now,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn remove_blob_if_unused(&mut self, hash: &BlobHash) -> Result<()> {
        let removed = self.conn.execute(
            "DELETE FROM blobs WHERE hash = ?1
               AND NOT EXISTS (SELECT 1 FROM laps WHERE blob_hash = ?1)",
            [hash.as_str()],
        )? > 0;
        if removed {
            self.blobs().remove(hash)?;
        }
        Ok(())
    }

    /// A lap's telemetry file as stored (for uploading).
    pub fn lap_blob(&self, hash: &BlobHash) -> Result<Vec<u8>> {
        self.blobs().get(hash)
    }

    /// Ids of the laps of a session (tests, diagnostics).
    pub fn lap_ids(&self, session: SessionId) -> Result<Vec<LapId>> {
        Ok(self.laps(session)?.into_iter().map(|l| l.id).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sy_model::channel::names;
    use sy_model::session::Conditions;
    use sy_model::{Channel, ChannelData, GameId, LapFlags, Origin, SessionType, Unit};
    use time::macros::datetime;

    fn session() -> Session {
        Session {
            id: SessionId::new(),
            game: GameId::Iracing,
            game_version: None,
            track_key: "iracing:1".into(),
            track_name: "T".into(),
            car_key: "iracing:2".into(),
            car_name: "C".into(),
            session_type: SessionType::Practice,
            started_at: datetime!(2026-11-10 18:00 UTC),
            ended_at: None,
            conditions: Conditions::default(),
            setup: None,
            session_info: None,
            origin: Origin::Own,
        }
    }

    fn add_lap(store: &mut Store, s: SessionId, n: u32) -> Lap {
        let t = LapTelemetry::new(10.0)
            .with_channel(Channel::new(
                names::SPEED,
                Unit::MeterPerSecond,
                ChannelData::F32(vec![n as f32; 20]),
            ))
            .unwrap();
        store
            .insert_lap(
                NewLap {
                    id: LapId::new(),
                    session_id: s,
                    number: n,
                    start_time_s: Some(f64::from(n) * 100.0),
                    lap_time_ms: Some(90_000),
                    sector_times_ms: vec![],
                    valid: true,
                    invalid_reason: None,
                    flags: LapFlags::default(),
                },
                &t,
            )
            .unwrap()
    }

    #[test]
    fn push_bookkeeping() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let s = session();
        store.insert_session(&s).unwrap();
        add_lap(&mut store, s.id, 1);
        add_lap(&mut store, s.id, 2);

        let p = store.pending_push(1).unwrap();
        assert_eq!((p.sessions.len(), p.laps.len()), (1, 1), "batched");
        store.mark_pushed(&p).unwrap();
        let p = store.pending_push(10).unwrap();
        assert_eq!((p.sessions.len(), p.laps.len()), (0, 1));
        store.mark_pushed(&p).unwrap();
        assert!(store.pending_push(10).unwrap().is_empty());

        // a change makes it pending again
        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut changed = s.clone();
        changed.ended_at = Some(datetime!(2026-11-10 19:00 UTC));
        store.update_session(&changed).unwrap();
        assert_eq!(store.pending_push(10).unwrap().sessions.len(), 1);

        // deleting a pushed session queues the deletion for the server
        store.delete_session(s.id).unwrap();
        let p = store.pending_push(10).unwrap();
        assert_eq!(p.deleted, vec![s.id]);
        store.mark_pushed(&p).unwrap();
        assert!(store.pending_push(10).unwrap().is_empty());

        // a session the server never had is deleted silently
        let other = session();
        store.insert_session(&other).unwrap();
        store.delete_session(other.id).unwrap();
        assert!(store.pending_push(10).unwrap().deleted.is_empty());
    }

    #[test]
    fn applying_pulled_data() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let mut src = Store::open(a.path()).unwrap();
        let mut dst = Store::open(b.path()).unwrap();
        let s = session();
        src.insert_session(&s).unwrap();
        let lap = add_lap(&mut src, s.id, 1);
        let bytes = src.lap_blob(&lap.blob_hash).unwrap();

        // a lap without its session is skipped
        assert!(dst.needs_telemetry(&lap).unwrap());
        assert!(!dst.apply_remote_lap(&lap, Some(&bytes)).unwrap());

        dst.apply_remote_session(&s).unwrap();
        assert!(dst.apply_remote_lap(&lap, Some(&bytes)).unwrap());
        assert_eq!(dst.lap(lap.id).unwrap(), lap);
        assert!(!dst.needs_telemetry(&lap).unwrap());
        assert!(!dst.apply_remote_lap(&lap, None).unwrap(), "unchanged");
        // pulled data is not pushed back
        assert!(dst.pending_push(10).unwrap().is_empty());
        // damaged telemetry is rejected
        let mut other = lap.clone();
        other.id = LapId::new();
        other.number = 2;
        other.start_time_s = Some(500.0);
        other.blob_hash = BlobHash::of(b"x");
        assert!(dst.apply_remote_lap(&other, Some(b"y")).is_err());

        // a remote deletion removes it without queueing it for the server
        dst.apply_remote_deletion(s.id).unwrap();
        assert!(dst.session(s.id).is_err());
        assert!(dst.pending_push(10).unwrap().deleted.is_empty());
        dst.apply_remote_deletion(s.id).unwrap(); // unknown: ignored
    }
}
