// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Laps other users shared on the server, kept locally as references
//! (docs/ARCHITECTURE.md §9.2, §9.5).
//!
//! Such sessions have the origin [`Origin::Synced`]. They are copies: never
//! pushed to the server, and deleted again when the owner stops sharing
//! ([`Store::remove_foreign_laps`]) or the app disconnects from the server
//! ([`Store::remove_all_foreign`]).

use rusqlite::params_from_iter;
use sy_model::remote::ForeignLap;
use sy_model::{Lap, LapId, Origin, Session, SessionId};

use crate::{Result, Store, StoreError};

const SYNCED: &str = "json_extract(origin, '$.kind') = 'synced'";

impl Store {
    /// Stores a session someone shared, or refreshes the copy. `laps` are
    /// the laps the user may see now; other laps of the copy are removed.
    ///
    /// Returns the id to store the laps under, or None if the user has this
    /// session themselves (recorded or imported): then nothing is changed.
    pub fn apply_shared_session(
        &mut self,
        s: &Session,
        laps: &[LapId],
    ) -> Result<Option<SessionId>> {
        if !matches!(s.origin, Origin::Synced { .. }) {
            return Err(StoreError::Corrupt {
                column: "sessions.origin",
                message: "a shared session must have the origin 'synced'".into(),
            });
        }
        if self.session_exists(s.id)? {
            if self.session(s.id)?.origin != s.origin {
                return Ok(None);
            }
            self.update_session(s)?;
            let gone: Vec<LapId> = self
                .laps(s.id)?
                .into_iter()
                .map(|l| l.id)
                .filter(|id| !laps.contains(id))
                .collect();
            self.remove_foreign_laps(&gone)?;
            // the session itself may have gone with its last lap
            if !self.session_exists(s.id)? {
                self.insert_session(s)?;
            }
        } else if self.canonical_session_id(s.id)? != s.id {
            return Ok(None);
        } else {
            self.insert_session(s)?;
        }
        Ok(Some(s.id))
    }

    /// Stores a lap of a shared session (see
    /// [`apply_shared_session`](Self::apply_shared_session)). `telemetry`
    /// is the downloaded file if [`needs_telemetry`](Self::needs_telemetry)
    /// said so. A lap the user has in another session is left alone
    /// (returns false).
    pub fn apply_shared_lap(&mut self, lap: &Lap, telemetry: Option<&[u8]>) -> Result<bool> {
        match self.lap(lap.id) {
            Ok(existing) if existing.session_id != lap.session_id => Ok(false),
            Ok(_) | Err(StoreError::LapNotFound(_)) => {
                self.apply_remote_lap_inner(lap, telemetry, false)
            }
            Err(e) => Err(e),
        }
    }

    /// The laps of other users kept here, with their owners' usernames.
    pub fn foreign_laps(&self) -> Result<Vec<ForeignLap>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT l.id, json_extract(s.origin, '$.owner')
             FROM laps l JOIN sessions s ON s.id = l.session_id
             WHERE {} ORDER BY l.id",
            SYNCED.replace("origin", "s.origin")
        ))?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, owner) = row?;
            out.push(ForeignLap {
                id: id.parse().map_err(|_| StoreError::Corrupt {
                    column: "laps.id",
                    message: id,
                })?,
                owner,
            });
        }
        Ok(out)
    }

    /// Deletes kept laps of other users (the owner stopped sharing them),
    /// and kept sessions that have no laps left. Own laps are never
    /// touched. Returns the number of deleted laps.
    pub fn remove_foreign_laps(&mut self, ids: &[LapId]) -> Result<u32> {
        if ids.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.transaction()?;
        let marks = vec!["?"; ids.len()].join(", ");
        let deleted = tx.execute(
            &format!(
                "DELETE FROM laps WHERE id IN ({marks})
                   AND session_id IN (SELECT id FROM sessions WHERE {SYNCED})"
            ),
            params_from_iter(ids.iter().map(ToString::to_string)),
        )?;
        tx.execute(
            &format!(
                "DELETE FROM sessions WHERE {SYNCED}
                   AND id NOT IN (SELECT session_id FROM laps)"
            ),
            [],
        )?;
        tx.commit()?;
        self.remove_orphan_blobs()?;
        Ok(deleted as u32)
    }

    /// Removes the setup from kept laps of other users (the owner stopped
    /// sharing it). Own laps are never touched. Returns the number of
    /// changed laps.
    pub fn remove_foreign_setups(&mut self, ids: &[LapId]) -> Result<u32> {
        if ids.is_empty() {
            return Ok(0);
        }
        let marks = vec!["?"; ids.len()].join(", ");
        let changed = self.conn.execute(
            &format!(
                "UPDATE laps SET setup = NULL WHERE id IN ({marks}) AND setup IS NOT NULL
                   AND session_id IN (SELECT id FROM sessions WHERE {SYNCED})"
            ),
            params_from_iter(ids.iter().map(ToString::to_string)),
        )?;
        Ok(changed as u32)
    }

    /// Deletes everything kept from other users (the app disconnects from
    /// the server, so access can't be checked anymore). Returns the number
    /// of deleted sessions.
    pub fn remove_all_foreign(&mut self) -> Result<u32> {
        let deleted = self
            .conn
            .execute(&format!("DELETE FROM sessions WHERE {SYNCED}"), [])?;
        self.conn.execute(
            "DELETE FROM sync_pushed WHERE kind = 'lap' AND id NOT IN (SELECT id FROM laps)",
            [],
        )?;
        self.remove_orphan_blobs()?;
        Ok(deleted as u32)
    }

    /// Removes telemetry files no lap refers to.
    fn remove_orphan_blobs(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
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
                self.blobs().remove(&hash)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sy_model::channel::names;
    use sy_model::session::Conditions;
    use sy_model::{
        Channel, ChannelData, GameId, LapFlags, LapTelemetry, NewLap, SessionType, Unit,
    };
    use time::macros::datetime;

    fn session(origin: Origin) -> Session {
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
            origin,
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
                    setup: None,
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

    fn synced() -> Origin {
        Origin::Synced {
            server: "https://laps.example".into(),
            owner: "ben".into(),
        }
    }

    #[test]
    fn keeping_and_revoking_shared_laps() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        // the owner's store, to produce laps with telemetry
        let mut theirs = Store::open(a.path()).unwrap();
        let own = session(Origin::Own);
        theirs.insert_session(&own).unwrap();
        let laps = [
            add_lap(&mut theirs, own.id, 1),
            add_lap(&mut theirs, own.id, 2),
        ];
        let bytes: Vec<Vec<u8>> = laps
            .iter()
            .map(|l| theirs.lap_blob(&l.blob_hash).unwrap())
            .collect();

        let mut mine = Store::open(b.path()).unwrap();
        let my_session = session(Origin::Own);
        mine.insert_session(&my_session).unwrap();
        let my_lap = add_lap(&mut mine, my_session.id, 7);

        let mut shared = own.clone();
        shared.origin = synced();
        let ids: Vec<LapId> = laps.iter().map(|l| l.id).collect();
        assert!(mine.apply_shared_session(&own, &ids).is_err(), "origin");
        assert_eq!(
            mine.apply_shared_session(&shared, &ids).unwrap(),
            Some(own.id)
        );
        for (lap, bytes) in laps.iter().zip(&bytes) {
            assert!(mine.needs_telemetry(lap).unwrap());
            assert!(mine.apply_shared_lap(lap, Some(bytes)).unwrap());
        }
        assert_eq!(mine.laps(own.id).unwrap().len(), 2);
        assert_eq!(
            mine.foreign_laps().unwrap().len(),
            2,
            "only the kept laps of others"
        );
        assert!(
            mine.foreign_laps()
                .unwrap()
                .iter()
                .all(|l| l.owner == "ben")
        );
        // copies are never pushed, and deleting one tells the server nothing
        let pending = mine.pending_push(100).unwrap();
        assert_eq!((pending.sessions.len(), pending.laps.len()), (1, 1));
        mine.mark_pushed(&pending).unwrap();

        // a setup shared later comes with a refresh; when it is withdrawn,
        // only the copy loses it, never an own lap
        let with_setup = Lap {
            setup: Some(serde_json::json!({"rear_wing": 9})),
            ..laps[0].clone()
        };
        assert!(mine.apply_shared_lap(&with_setup, None).unwrap());
        let mut own_with_setup = my_lap.clone();
        own_with_setup.setup = Some(serde_json::json!({"rear_wing": 7}));
        mine.conn
            .execute(
                "UPDATE laps SET setup = '{\"rear_wing\":7}' WHERE id = ?1",
                [my_lap.id.to_string()],
            )
            .unwrap();
        assert_eq!(
            mine.remove_foreign_setups(&[ids[0], ids[1], my_lap.id])
                .unwrap(),
            1
        );
        assert_eq!(mine.lap(ids[0]).unwrap().setup, None);
        assert_eq!(mine.lap(my_lap.id).unwrap(), own_with_setup);

        // refreshing with fewer laps removes the others
        assert_eq!(
            mine.apply_shared_session(&shared, &ids[..1]).unwrap(),
            Some(own.id)
        );
        assert_eq!(mine.lap_ids(own.id).unwrap(), vec![ids[0]]);
        assert!(!mine.blobs().contains(&laps[1].blob_hash));

        // revocation: own laps are never touched
        assert_eq!(mine.remove_foreign_laps(&[ids[0], my_lap.id]).unwrap(), 1);
        assert!(mine.session(own.id).is_err(), "empty copy is removed");
        assert!(mine.lap(my_lap.id).is_ok());
        assert!(mine.blobs().contains(&my_lap.blob_hash));
        assert!(!mine.blobs().contains(&laps[0].blob_hash));
        assert!(mine.pending_push(100).unwrap().is_empty());

        // disconnecting removes all copies
        mine.apply_shared_session(&shared, &ids).unwrap();
        mine.apply_shared_lap(&laps[0], Some(&bytes[0])).unwrap();
        assert_eq!(mine.remove_all_foreign().unwrap(), 1);
        assert_eq!(mine.list_sessions().unwrap().len(), 1);
        assert!(mine.pending_push(100).unwrap().is_empty());
    }

    #[test]
    fn own_copy_of_a_shared_session_wins() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let imported = session(Origin::Imported {
            driver_name: Some("Ben".into()),
        });
        store.insert_session(&imported).unwrap();
        let lap = add_lap(&mut store, imported.id, 1);
        let mut shared = imported.clone();
        shared.origin = synced();
        assert_eq!(
            store.apply_shared_session(&shared, &[lap.id]).unwrap(),
            None
        );
        assert_eq!(store.session(imported.id).unwrap().origin, imported.origin);
        assert_eq!(store.remove_all_foreign().unwrap(), 0);
        assert_eq!(store.laps(imported.id).unwrap().len(), 1);
    }
}
