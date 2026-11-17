// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The Linux reader of `LMU_Data` in `/dev/shm`: a file written the way the
//! game writes it when another shared-memory bridge backs its mapping with
//! that file, read while it is being written.

#![cfg(unix)]

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sy_model::capture::RawSource;
use sy_source_lmu::layout::{
    OBJECT_SIZE, OFF_GAME_VERSION, OFF_SCORING_INFO, OFF_TELEM_INFO, OFF_TELEMETRY,
    OFF_VEH_SCORING, SCORING_INFO_SIZE, VEH_SCORING_SIZE,
};
use sy_source_lmu::records::{KIND_SCORING, KIND_TELEMETRY, Scoring, Telemetry};
use sy_source_lmu::sim::SimLmu;
use sy_source_lmu::{LiveSource, Probe};

/// Writes `frames` telemetry frames (100 Hz game time), one every `pace`.
fn write_session(file: &File, frames: usize, pace: Duration) -> Vec<f64> {
    let mut sim = SimLmu::new();
    let mut next_scoring = sim.et;
    let mut written = Vec::new();
    file.write_all_at(&1_200i32.to_le_bytes(), OFF_GAME_VERSION as u64)
        .unwrap();
    for _ in 0..frames {
        let _ = sim.drive(0.01);
        if sim.et + 1e-9 >= next_scoring {
            next_scoring += 0.2;
            let rec = sim.scoring();
            let p = &rec.payload[12..];
            file.write_all_at(&p[..SCORING_INFO_SIZE], OFF_SCORING_INFO as u64)
                .unwrap();
            file.write_all_at(
                &p[SCORING_INFO_SIZE..][..VEH_SCORING_SIZE],
                OFF_VEH_SCORING as u64,
            )
            .unwrap();
        }
        // numVehicles active, playerVehicleIdx 0, playerHasVehicle
        file.write_all_at(&[1, 0, 1], OFF_TELEMETRY as u64).unwrap();
        file.write_all_at(&sim.telemetry(), OFF_TELEM_INFO as u64)
            .unwrap();
        written.push(sim.et);
        std::thread::sleep(pace);
    }
    written
}

#[test]
fn reads_what_the_game_writes_into_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("LMU_Data");
    let file = File::create(&path).unwrap();
    file.set_len(OBJECT_SIZE as u64).unwrap();

    let done = Arc::new(AtomicBool::new(false));
    let writer = {
        let done = done.clone();
        std::thread::spawn(move || {
            let written = write_session(&file, 300, Duration::from_millis(5));
            done.store(true, Ordering::SeqCst);
            written
        })
    };

    let mut source = LiveSource::at(&path);
    let mut telemetry = Vec::new();
    let mut scoring = 0;
    let mut quiet_since: Option<Instant> = None;
    loop {
        match source.next_record(Duration::from_millis(50)).unwrap() {
            Some(r) if r.kind == KIND_TELEMETRY => {
                let t = Telemetry::parse(&r.payload).unwrap();
                // a torn copy would mix fields of two frames
                assert!(
                    (t.fuel - (60.0 - t.elapsed_time * 0.01)).abs() < 1e-9,
                    "fuel {} doesn't belong to et {}",
                    t.fuel,
                    t.elapsed_time
                );
                telemetry.push(t.elapsed_time);
                quiet_since = None;
            }
            Some(r) if r.kind == KIND_SCORING => {
                let s = Scoring::parse(&r.payload).unwrap();
                assert!(s.in_realtime);
                scoring += 1;
            }
            Some(_) => {}
            None if done.load(Ordering::SeqCst)
                && quiet_since.get_or_insert_with(Instant::now).elapsed()
                    > Duration::from_millis(200) =>
            {
                break;
            }
            None => {}
        }
    }
    let written = writer.join().unwrap();

    assert!(
        telemetry.windows(2).all(|w| w[0] < w[1]),
        "elapsed time goes forward"
    );
    assert_eq!(telemetry.last(), written.last(), "the last frame arrives");
    // polled, not signalled: a few frames may be skipped, but not many
    assert!(
        telemetry.len() * 10 >= written.len() * 9,
        "{} of {} frames read",
        telemetry.len(),
        written.len()
    );
    assert!(scoring >= 10, "{scoring} scoring records");
    assert!(
        source.driving().load(Ordering::Relaxed),
        "driving while the data moves"
    );
}

#[test]
fn a_file_smaller_than_the_layout_is_an_unknown_layout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("LMU_Data");
    File::create(&path)
        .unwrap()
        .set_len(OBJECT_SIZE as u64 / 2)
        .unwrap();
    assert_eq!(LiveSource::probe_at(&path), Probe::UnknownLayout);
}

#[test]
fn nothing_is_read_from_a_missing_or_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("LMU_Data");
    let mut missing = LiveSource::at(&path);
    assert!(
        missing
            .next_record(Duration::from_millis(20))
            .unwrap()
            .is_none()
    );

    File::create(&path).unwrap();
    let mut empty = LiveSource::at(&path);
    assert!(
        empty
            .next_record(Duration::from_millis(20))
            .unwrap()
            .is_none()
    );
    assert_eq!(LiveSource::probe_at(&path), Probe::UnknownLayout);
}
