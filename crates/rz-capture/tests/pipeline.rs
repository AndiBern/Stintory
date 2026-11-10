// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pipeline tests with real iRacing recordings (skipped without them, see
//! tests/iracing_ibt.rs).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rz_capture::RawSource;
use rz_capture::pipeline::{RunOptions, SharedStore, recover_spools, run};
use rz_capture::spool::SpoolWriter;
use rz_source_iracing::IbtSource;
use rz_store::{KEEP_RAW_RECORDINGS, SpoolState, Store};

mod common;
use common::LiveLike;

fn example(needle: &str) -> Option<Vec<PathBuf>> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../telemetry_examples/iRacing");
    let files: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().contains(needle))
        .collect();
    (!files.is_empty()).then_some(files)
}

fn opts<'a>() -> RunOptions<'a> {
    RunOptions {
        spool: None,
        should_stop: &|| false,
        idle_done: None,
        progress: None,
    }
}

fn shared(dir: &std::path::Path) -> SharedStore {
    SharedStore(Arc::new(Mutex::new(Store::open(dir).unwrap())))
}

fn lap_counts(store: &SharedStore) -> Vec<(String, u32, Option<u32>)> {
    store.with(|s| {
        s.list_sessions()
            .unwrap()
            .into_iter()
            .map(|x| (x.track_name, x.lap_count, x.best_lap_ms))
            .collect()
    })
}

#[test]
fn import_is_idempotent() {
    let Some(files) = example("snetterton") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mut store = shared(dir.path());
    let first = run(
        &mut IbtSource::open_many(&files).unwrap(),
        &mut store,
        opts(),
    )
    .unwrap();
    let after_first = lap_counts(&store);
    let second = run(
        &mut IbtSource::open_many(&files).unwrap(),
        &mut store,
        opts(),
    )
    .unwrap();
    println!("{after_first:?} {first:?} {second:?}");
    assert_eq!(after_first.len(), 2, "qualifying + race");
    assert_eq!(lap_counts(&store), after_first, "no duplicates");
    assert!(
        after_first
            .iter()
            .all(|(name, _, _)| name.starts_with("Snetterton"))
    );
    // race best lap: 125.172 s (valid, official)
    assert!(
        after_first
            .iter()
            .any(|(_, _, best)| *best == Some(125_172))
    );
}

/// Recording live, then importing the game's file of the same session must
/// not create a duplicate. The laps get the file's richer telemetry (GPS).
#[test]
fn import_after_live_recording_merges() {
    let Some(files) = example("winton national 2026-11-10 17-00-40") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mut store = shared(dir.path());
    // the live recording saw the session start 30 s later: another
    // offline session key, so other ids
    let mut live = LiveLike::new(IbtSource::open_many(&files).unwrap()).with_start_shifted(30.0);
    run(&mut live, &mut store, opts()).unwrap();
    let after_live = lap_counts(&store);
    assert_eq!(after_live.len(), 1);

    run(
        &mut IbtSource::open_many(&files).unwrap(),
        &mut store,
        opts(),
    )
    .unwrap();
    assert_eq!(lap_counts(&store), after_live, "merged, no duplicate");
    store.with(|s| {
        let id = s.list_sessions().unwrap()[0].id;
        for lap in s.laps(id).unwrap() {
            let t = s.lap_telemetry(lap.id).unwrap();
            assert!(
                t.channel("iracing.lat").is_some(),
                "lap {}: GPS from file",
                lap.number
            );
        }
    });
}

/// Writes all raw records of `files` into a spool (like a live recording
/// that crashed before processing).
fn record_spool(store: &SharedStore, files: &[PathBuf]) -> rz_store::SpoolEntry {
    let entry = store.with(|s| s.register_spool(rz_model::GameId::Iracing).unwrap());
    let mut writer = SpoolWriter::create(&entry.path, rz_model::GameId::Iracing).unwrap();
    let mut src = IbtSource::open_many(files).unwrap();
    while let Some(rec) = src.next_record(Duration::ZERO).unwrap() {
        writer.append(rec.kind, &rec.payload).unwrap();
    }
    writer.finish().unwrap();
    entry
}

#[test]
fn crash_recovery_replays_spool_and_deletes_it() {
    let Some(files) = example("oschersleben gp 2026-11-09 14-27-25") else {
        return;
    };
    // reference: direct import
    let ref_dir = tempfile::tempdir().unwrap();
    let mut reference = shared(ref_dir.path());
    run(
        &mut IbtSource::open_many(&files).unwrap(),
        &mut reference,
        opts(),
    )
    .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let store = shared(dir.path());
    let spool = record_spool(&store, &files);
    let size = std::fs::metadata(&spool.path).unwrap().len();
    println!("spool size {} KB for {} rows", size / 1024, 55_577);

    let recovered = recover_spools(&store).unwrap();
    assert_eq!(recovered, 10);
    assert_eq!(lap_counts(&store), lap_counts(&reference));
    assert!(!spool.path.exists(), "processed spool is deleted");
    assert!(
        store
            .with(|s| s
                .spools(&[SpoolState::Recording, SpoolState::Processed])
                .unwrap())
            .is_empty()
    );
}

#[test]
fn keep_raw_recordings_keeps_the_file() {
    let Some(files) = example("oschersleben gp 2026-11-09 14-19-02") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = shared(dir.path());
    store.with(|s| s.set_setting(KEEP_RAW_RECORDINGS, &true).unwrap());
    let spool = record_spool(&store, &files);
    recover_spools(&store).unwrap();
    assert!(spool.path.exists());
    let processed = store.with(|s| s.spools(&[SpoolState::Processed]).unwrap());
    assert_eq!(processed.len(), 1);
}
