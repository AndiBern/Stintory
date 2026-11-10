// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Export to and import from `.rzx` files (docs/PLAN.md §8).

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use rz_model::rzx::{RzxReader, RzxWriter};
use rz_model::{Lap, LapId, LapTelemetry, Origin, Session, SessionId};
use serde_json::Value;

use crate::{Result, Store, StoreError};

/// What to export: whole sessions and/or single laps (with their session).
#[derive(Debug, Clone, Default)]
pub struct ExportSelection {
    /// Sessions with all their laps
    pub sessions: Vec<SessionId>,
    /// Single laps
    pub laps: Vec<LapId>,
}

/// How to export.
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    /// Program name and version for the manifest
    pub exporter: String,
    /// Driver name shown to whoever imports the file (None = anonymous)
    pub driver_name: Option<String>,
    /// Include car setups (off by default: setups are often private)
    pub include_setup: bool,
}

/// Result of an export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportStats {
    /// Sessions written
    pub sessions: u32,
    /// Laps written
    pub laps: u32,
}

/// Result of an import.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BundleImport {
    /// New sessions
    pub sessions: u32,
    /// New laps
    pub laps: u32,
    /// Laps that were already stored
    pub skipped_laps: u32,
}

impl Store {
    /// Writes the selected sessions and laps to an `.rzx` file at `path`.
    ///
    /// The file is written next to `path` first and renamed when complete,
    /// so a failed export never leaves a half-written file behind.
    ///
    /// Privacy: setups are left out unless [`ExportOptions::include_setup`],
    /// and the game's player identity (name, customer id) is removed from the
    /// session info. The driver name in the file is the one the user chose.
    pub fn export_rzx(
        &self,
        path: &Path,
        selection: &ExportSelection,
        options: &ExportOptions,
    ) -> Result<ExportStats> {
        let mut chosen: BTreeMap<SessionId, Vec<Lap>> = BTreeMap::new();
        for &id in &selection.sessions {
            self.session(id)?; // must exist, even without laps
            chosen.insert(id, self.laps(id)?);
        }
        for &id in &selection.laps {
            let lap = self.lap(id)?;
            let laps = chosen.entry(lap.session_id).or_default();
            if !laps.iter().any(|l| l.id == lap.id) {
                laps.push(lap);
            }
        }
        if chosen.is_empty() {
            return Err(StoreError::NothingToExport);
        }

        let tmp = part_path(path);
        let result = (|| {
            let file = File::create(&tmp)?;
            let mut writer = RzxWriter::new(BufWriter::new(file));
            let mut stats = ExportStats {
                sessions: 0,
                laps: 0,
            };
            for (&id, laps) in &mut chosen {
                let session = for_export(self.session(id)?, options.include_setup);
                writer.add_session(&session)?;
                stats.sessions += 1;
                laps.sort_by_key(|l| l.number);
                for lap in laps.iter() {
                    let bytes = self.blobs().get(&lap.blob_hash)?;
                    writer.add_lap(lap, &bytes)?;
                    stats.laps += 1;
                }
            }
            let mut out = writer.finish(&options.exporter, options.driver_name.as_deref())?;
            out.flush()?;
            out.into_inner()
                .map_err(|e| StoreError::Io(e.into_error()))?
                .sync_all()?;
            std::fs::rename(&tmp, path)?;
            Ok(stats)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    /// Imports an `.rzx` file. Sessions and laps that are already stored (same
    /// id) are kept as they are; new ones get the origin "imported" with the
    /// driver name from the file.
    pub fn import_rzx(&mut self, path: &Path) -> Result<BundleImport> {
        let mut reader = RzxReader::open(BufReader::new(File::open(path)?))?;
        let driver_name = reader.manifest().driver_name.clone();
        let sessions = reader.sessions()?;
        let laps = reader.laps()?;

        let mut stats = BundleImport::default();
        let mut target: HashMap<SessionId, SessionId> = HashMap::new();
        for mut session in sessions {
            let id = self.canonical_session_id(session.id)?;
            if self.session_exists(id)? {
                target.insert(session.id, id); // ours already, or imported before
                continue;
            }
            session.origin = imported_origin(session.origin, &driver_name);
            self.insert_session(&session)?;
            target.insert(session.id, session.id);
            stats.sessions += 1;
        }
        for mut lap in laps {
            match self.lap(lap.id) {
                Ok(_) => {
                    stats.skipped_laps += 1;
                    continue;
                }
                Err(StoreError::LapNotFound(_)) => {}
                Err(e) => return Err(e),
            }
            let bytes = reader.telemetry(&lap)?;
            // must be telemetry we can read, not just any bytes with a hash
            let telemetry = LapTelemetry::from_parquet(bytes.clone())?;
            lap.sample_count = u32::try_from(telemetry.len()).unwrap_or(u32::MAX);
            lap.session_id = target[&lap.session_id];
            let hash = self.blobs().put(&bytes)?;
            debug_assert_eq!(hash, lap.blob_hash);
            self.insert_lap_row(&lap, bytes.len())?;
            stats.laps += 1;
        }
        Ok(stats)
    }
}

/// `<path>.part`
fn part_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    path.with_file_name(name)
}

/// A session as it goes into an export file.
fn for_export(mut session: Session, include_setup: bool) -> Session {
    if !include_setup {
        session.setup = None;
    }
    if let Some(Value::Object(info)) = session.session_info.as_mut() {
        if !include_setup {
            info.remove("setup");
        }
        if let Some(Value::Object(player)) = info.get_mut("player") {
            player.remove("user_id");
            player.remove("user_name");
        }
    }
    session
}

/// The origin of a session imported from a file with this driver name.
fn imported_origin(origin: Origin, driver_name: &Option<String>) -> Origin {
    match origin {
        Origin::Own => Origin::Imported {
            driver_name: driver_name.clone(),
        },
        // passed on: keep who drove it
        Origin::Imported { driver_name } => Origin::Imported { driver_name },
        Origin::Synced { owner, .. } => Origin::Imported {
            driver_name: Some(owner),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rz_model::channel::names;
    use rz_model::session::Conditions;
    use rz_model::{Channel, ChannelData, GameId, LapFlags, NewLap, SessionType, Unit};
    use time::macros::datetime;

    fn session(store: &mut Store) -> Session {
        let s = Session {
            id: SessionId::new(),
            game: GameId::Iracing,
            game_version: None,
            track_key: "iracing:439".into(),
            track_name: "Winton".into(),
            car_key: "iracing:67".into(),
            car_name: "MX-5".into(),
            session_type: SessionType::Practice,
            started_at: datetime!(2026-11-10 16:00 UTC),
            ended_at: None,
            conditions: Conditions::default(),
            setup: Some(serde_json::json!({"brake_bias": 0.56})),
            session_info: Some(serde_json::json!({
                "player": {"user_id": 4242, "user_name": "Real Name", "car_id": 67},
                "track": {"id": 439}
            })),
            origin: Origin::Own,
        };
        store.insert_session(&s).unwrap();
        s
    }

    fn lap(store: &mut Store, s: &Session, number: u32) -> Lap {
        let t = LapTelemetry::new(60.0)
            .with_channel(Channel::new(
                names::SPEED,
                Unit::MeterPerSecond,
                ChannelData::F32(vec![number as f32; 30]),
            ))
            .unwrap();
        store
            .insert_lap(
                NewLap {
                    id: LapId::new(),
                    session_id: s.id,
                    number,
                    start_time_s: Some(f64::from(number) * 95.0),
                    lap_time_ms: Some(95_000 + number),
                    sector_times_ms: vec![],
                    valid: true,
                    invalid_reason: None,
                    flags: LapFlags::default(),
                },
                &t,
            )
            .unwrap()
    }

    fn options(include_setup: bool) -> ExportOptions {
        ExportOptions {
            exporter: "test".into(),
            driver_name: Some("Andi".into()),
            include_setup,
        }
    }

    #[test]
    fn export_then_import_elsewhere() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let mut src = Store::open(a.path()).unwrap();
        let s = session(&mut src);
        let laps: Vec<Lap> = (1..=3).map(|n| lap(&mut src, &s, n)).collect();
        let file = a.path().join("export.rzx");
        let stats = src
            .export_rzx(
                &file,
                &ExportSelection {
                    sessions: vec![s.id],
                    laps: vec![],
                },
                &options(false),
            )
            .unwrap();
        assert_eq!(
            stats,
            ExportStats {
                sessions: 1,
                laps: 3
            }
        );
        assert!(!part_path(&file).exists());

        let mut dst = Store::open(b.path()).unwrap();
        let imported = dst.import_rzx(&file).unwrap();
        assert_eq!(
            imported,
            BundleImport {
                sessions: 1,
                laps: 3,
                skipped_laps: 0
            }
        );
        let got = dst.session(s.id).unwrap();
        assert_eq!(
            got.origin,
            Origin::Imported {
                driver_name: Some("Andi".into())
            }
        );
        assert_eq!(got.setup, None, "setup not exported by default");
        let info = got.session_info.unwrap().to_string();
        assert!(
            !info.contains("Real Name") && !info.contains("4242"),
            "{info}"
        );
        assert!(info.contains("\"car_id\":67"));
        assert_eq!(dst.laps(s.id).unwrap(), laps, "same ids, times, hashes");
        assert_eq!(
            dst.lap_telemetry(laps[1].id).unwrap(),
            src.lap_telemetry(laps[1].id).unwrap()
        );

        // importing again changes nothing
        assert_eq!(
            dst.import_rzx(&file).unwrap(),
            BundleImport {
                sessions: 0,
                laps: 0,
                skipped_laps: 3
            }
        );
        // importing our own export back: ours stays "own"
        src.import_rzx(&file).unwrap();
        assert_eq!(src.session(s.id).unwrap().origin, Origin::Own);
    }

    #[test]
    fn export_single_laps_with_setup() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let s = session(&mut store);
        let laps: Vec<Lap> = (1..=3).map(|n| lap(&mut store, &s, n)).collect();
        let file = dir.path().join("laps.rzx");
        let stats = store
            .export_rzx(
                &file,
                &ExportSelection {
                    sessions: vec![],
                    laps: vec![laps[2].id, laps[0].id, laps[0].id],
                },
                &options(true),
            )
            .unwrap();
        assert_eq!(
            stats,
            ExportStats {
                sessions: 1,
                laps: 2
            }
        );
        let mut r = RzxReader::open(File::open(&file).unwrap()).unwrap();
        assert!(r.sessions().unwrap()[0].setup.is_some());
        let numbers: Vec<u32> = r.laps().unwrap().iter().map(|l| l.number).collect();
        assert_eq!(numbers, [1, 3]);
    }

    #[test]
    fn empty_selection_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert!(matches!(
            store.export_rzx(
                &dir.path().join("x.rzx"),
                &ExportSelection::default(),
                &options(false)
            ),
            Err(StoreError::NothingToExport)
        ));
    }

    #[test]
    fn imported_session_is_not_merged_with_own_one() {
        // a team mate drove the same hosted session: same track, car, type
        // and start; it must stay a separate session
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let mut mate = Store::open(a.path()).unwrap();
        let theirs = session(&mut mate);
        lap(&mut mate, &theirs, 1);
        let file = a.path().join("mate.rzx");
        mate.export_rzx(
            &file,
            &ExportSelection {
                sessions: vec![theirs.id],
                laps: vec![],
            },
            &options(false),
        )
        .unwrap();

        let mut me = Store::open(b.path()).unwrap();
        let mine = session(&mut me);
        lap(&mut me, &mine, 1);
        me.import_rzx(&file).unwrap();
        assert_eq!(me.list_sessions().unwrap().len(), 2);
        // and our next recording of "the same" session still merges with ours
        let mut again = mine.clone();
        again.id = SessionId::new();
        assert_eq!(me.upsert_session(&again).unwrap(), mine.id);
    }
}
