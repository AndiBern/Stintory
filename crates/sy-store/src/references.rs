// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Laps to compare with for a track and car (docs/ARCHITECTURE.md §7.4, §7.5):
//! own recordings, imported laps and the kept copies of laps others shared.

use rusqlite::params;
use sy_model::Origin;
use sy_model::api::{REFERENCE_LAPS_MAX, REFERENCE_LAPS_PER_DRIVER, ReferenceLap, ReferenceScope};

use crate::store::{json, parse};
use crate::{Result, Store, StoreError, timefmt};

impl Store {
    /// Laps of `track_key` + `car_key` that count for a best lap, fastest
    /// first, at most [`REFERENCE_LAPS_PER_DRIVER`] per driver (the user,
    /// each imported driver, each owner of kept shared laps). `owner` is
    /// never set here: local copies only know the owner's username (in
    /// the origin).
    pub fn reference_laps(
        &self,
        track_key: &str,
        car_key: &str,
        scope: ReferenceScope,
    ) -> Result<Vec<ReferenceLap>> {
        let (mine, shared) = match scope {
            ReferenceScope::All => (true, true),
            ReferenceScope::Mine => (true, false),
            ReferenceScope::Shared => (false, true),
        };
        let mut stmt = self.conn.prepare(
            "WITH candidates AS (
                SELECT l.id, l.session_id, l.number, l.lap_time_ms, l.sector_times_ms,
                       s.game, s.track_key, s.track_name, s.car_key, s.car_name,
                       s.session_type, s.started_at, s.origin,
                       row_number() OVER (
                           PARTITION BY json_extract(s.origin, '$.kind'),
                                        COALESCE(json_extract(s.origin, '$.driver_name'), ''),
                                        COALESCE(json_extract(s.origin, '$.owner'), '')
                           ORDER BY l.lap_time_ms, s.started_at, l.id) AS rank
                FROM laps l JOIN sessions s ON s.id = l.session_id
                WHERE s.track_key = ?1 AND s.car_key = ?2
                  AND l.lap_time_ms IS NOT NULL AND l.valid = 1
                  AND COALESCE(json_extract(l.flags, '$.out_lap'), 0) = 0
                  AND COALESCE(json_extract(l.flags, '$.in_lap'), 0) = 0
                  AND COALESCE(json_extract(l.flags, '$.incomplete'), 0) = 0
                  AND CASE json_extract(s.origin, '$.kind') WHEN 'synced' THEN ?4 ELSE ?3 END
             )
             SELECT * FROM candidates WHERE rank <= ?5
             ORDER BY lap_time_ms, started_at, id
             LIMIT ?6",
        )?;
        let rows = stmt.query_map(
            params![
                track_key,
                car_key,
                mine,
                shared,
                REFERENCE_LAPS_PER_DRIVER as i64,
                REFERENCE_LAPS_MAX as i64
            ],
            |row| {
                Ok((|| {
                    let origin: Origin = json(row, 12)?.ok_or(StoreError::Corrupt {
                        column: "sessions.origin",
                        message: "NULL".into(),
                    })?;
                    Ok(ReferenceLap {
                        lap_id: parse(row, 0, "laps.id")?,
                        session_id: parse(row, 1, "laps.session_id")?,
                        number: row.get(2)?,
                        lap_time_ms: row.get(3)?,
                        sector_times_ms: json(row, 4)?.unwrap_or_default(),
                        game: parse(row, 5, "sessions.game")?,
                        track_key: row.get(6)?,
                        track_name: row.get(7)?,
                        car_key: row.get(8)?,
                        car_name: row.get(9)?,
                        session_type: parse(row, 10, "sessions.session_type")?,
                        session_started_at: timefmt::decode(
                            "sessions.started_at",
                            &row.get::<_, String>(11)?,
                        )?,
                        origin: origin.into(),
                        owner: None,
                        remote: false,
                    })
                })())
            },
        )?;
        rows.map(|r| r?).collect()
    }
}

#[cfg(test)]
mod tests {
    use sy_model::api::OriginView;
    use sy_model::channel::names;
    use sy_model::session::Conditions;
    use sy_model::{
        Channel, ChannelData, GameId, LapFlags, LapId, LapTelemetry, NewLap, Session, SessionId,
        SessionType, Unit,
    };
    use time::macros::datetime;

    use super::*;

    fn session(store: &mut Store, origin: Origin, car: &str) -> SessionId {
        let s = Session {
            id: SessionId::new(),
            game: GameId::Iracing,
            game_version: None,
            track_key: "iracing:1".into(),
            track_name: "T".into(),
            car_key: car.into(),
            car_name: "C".into(),
            session_type: SessionType::Practice,
            started_at: datetime!(2026-11-10 18:00 UTC),
            ended_at: None,
            conditions: Conditions::default(),
            setup: None,
            session_info: None,
            origin,
        };
        store.insert_session(&s).unwrap();
        s.id
    }

    fn lap(store: &mut Store, s: SessionId, n: u32, ms: u32, flags: LapFlags) -> LapId {
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
                    lap_time_ms: Some(ms),
                    sector_times_ms: vec![],
                    valid: true,
                    invalid_reason: None,
                    flags,
                },
                &t,
            )
            .unwrap()
            .id
    }

    #[test]
    fn fastest_clean_laps_per_driver() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let own = session(&mut store, Origin::Own, "iracing:2");
        for n in 1..=12 {
            lap(&mut store, own, n, 90_000 + n, LapFlags::default());
        }
        let out = LapFlags {
            out_lap: true,
            ..LapFlags::default()
        };
        lap(&mut store, own, 13, 80_000, out);
        let ben = session(
            &mut store,
            Origin::Synced {
                server: "https://laps.example".into(),
                owner: "ben".into(),
            },
            "iracing:2",
        );
        let ben_lap = lap(&mut store, ben, 1, 89_000, LapFlags::default());
        let imported = session(
            &mut store,
            Origin::Imported {
                driver_name: Some("Max".into()),
            },
            "iracing:2",
        );
        lap(&mut store, imported, 1, 95_000, LapFlags::default());
        let other_car = session(&mut store, Origin::Own, "iracing:3");
        lap(&mut store, other_car, 1, 70_000, LapFlags::default());

        let all = store
            .reference_laps("iracing:1", "iracing:2", ReferenceScope::All)
            .unwrap();
        assert_eq!(all.len(), REFERENCE_LAPS_PER_DRIVER + 2, "10 own, ben, max");
        assert_eq!(all[0].lap_id, ben_lap);
        assert!(matches!(&all[0].origin, OriginView::Synced { owner, .. } if owner == "ben"));
        assert_eq!(all[1].lap_time_ms, 90_001, "the out-lap doesn't count");
        assert!(all.windows(2).all(|w| w[0].lap_time_ms <= w[1].lap_time_ms));
        assert!(all.iter().all(|l| l.owner.is_none() && !l.remote));

        let shared = store
            .reference_laps("iracing:1", "iracing:2", ReferenceScope::Shared)
            .unwrap();
        assert_eq!(shared.len(), 1);
        let mine = store
            .reference_laps("iracing:1", "iracing:2", ReferenceScope::Mine)
            .unwrap();
        assert_eq!(mine.len(), REFERENCE_LAPS_PER_DRIVER + 1);
        assert!(
            mine.iter()
                .all(|l| !matches!(l.origin, OriginView::Synced { .. }))
        );
    }
}
