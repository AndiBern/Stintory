// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! End-to-end check of the iRacing pipeline with real `.ibt` recordings:
//! IbtSource → IracingDecoder → Recorder.
//!
//! The recordings are personal data and not part of the repository. Put them
//! into `<repo>/telemetry_examples/iRacing/`; without them the tests are
//! skipped.

use std::path::PathBuf;
use std::time::Duration;

use sy_capture::{Decoder, RawSource, RecordedLap, Recorder, RecorderConfig, RecorderOutput};
use sy_model::Session;
use sy_model::channel::names;
use sy_source_iracing::{IbtSource, IracingDecoder};

mod common;
use common::LiveLike;

fn examples() -> Option<Vec<PathBuf>> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../telemetry_examples/iRacing");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "ibt"))
        .collect();
    files.sort();
    (!files.is_empty()).then_some(files)
}

fn files_matching(all: &[PathBuf], needle: &str) -> Vec<PathBuf> {
    all.iter()
        .filter(|p| p.to_string_lossy().contains(needle))
        .cloned()
        .collect()
}

struct Result {
    sessions: Vec<Session>,
    laps: Vec<RecordedLap>,
}

fn run(paths: &[PathBuf]) -> Result {
    let mut source = IbtSource::open_many(paths).expect("open ibt");
    run_source(&mut source)
}

fn run_source(source: &mut dyn RawSource) -> Result {
    let mut decoder = IracingDecoder::new();
    let mut recorder = Recorder::new(RecorderConfig {
        official_time_wait_s: 8.0,
        ..RecorderConfig::default()
    });
    let mut out = Vec::new();
    while let Some(rec) = source.next_record(Duration::ZERO).expect("read") {
        for event in decoder.decode(&rec).expect("decode") {
            out.extend(recorder.handle(event));
        }
    }
    for event in decoder.finish() {
        out.extend(recorder.handle(event));
    }
    out.extend(recorder.finish());

    let mut sessions: Vec<Session> = Vec::new();
    let mut laps = Vec::new();
    for o in out {
        match o {
            RecorderOutput::SessionStarted(s)
            | RecorderOutput::SessionUpdated(s)
            | RecorderOutput::SessionEnded(s) => match sessions.iter_mut().find(|x| x.id == s.id) {
                Some(x) => *x = s,
                None => sessions.push(s),
            },
            RecorderOutput::Lap(l) => laps.push(*l),
        }
    }
    Result { sessions, laps }
}

fn print(r: &Result) {
    for s in &r.sessions {
        println!(
            "session {} {} {} {:?} {} → {:?}",
            s.id, s.track_key, s.car_key, s.session_type, s.started_at, s.ended_at
        );
    }
    for l in &r.laps {
        println!(
            "  lap {:>2} {:>9} valid={:<5} {:?} {:?} samples={}",
            l.lap.number,
            l.lap
                .lap_time_ms
                .map_or("-".into(), |t| format!("{:.3}", t as f64 / 1000.0)),
            l.lap.valid,
            l.lap.invalid_reason.as_deref().unwrap_or(""),
            l.lap.flags,
            l.telemetry.len()
        );
    }
}

fn lap(r: &Result, number: u32, time_ms: u32) -> &RecordedLap {
    r.laps
        .iter()
        .find(|l| l.lap.number == number && l.lap.lap_time_ms == Some(time_ms))
        .unwrap_or_else(|| panic!("lap {number} with {time_ms} ms not found"))
}

#[test]
fn oschersleben_race_matches_official_times() {
    let Some(all) = examples() else {
        eprintln!("no iRacing examples, skipped");
        return;
    };
    let files = files_matching(&all, "oschersleben gp 2026-11-09 14-27-25");
    if files.is_empty() {
        return;
    }
    let r = run(&files);
    print(&r);

    assert_eq!(r.sessions.len(), 1);
    let s = &r.sessions[0];
    assert_eq!(s.track_key, "iracing:449");
    assert_eq!(s.car_key, "iracing:163");
    assert_eq!(s.session_type, sy_model::SessionType::Race);
    assert!(s.ended_at.is_some());

    // official times reported by iRacing (LapLastLapTime)
    assert!(lap(&r, 1, 100_030).lap.valid);
    // In races iRacing counts every lap, but laps with an off-track moment
    // are not clean reference laps for us.
    for (n, t) in [(7, 93_586), (8, 93_903)] {
        let l = &lap(&r, n, t).lap;
        assert!(!l.valid);
        assert!(l.invalid_reason.as_deref().unwrap().contains("off track"));
    }
    // After the incidents in lap 2 the meatball flag was out until the pit
    // stop in lap 6: laps 2-5 don't count (iRacing kept LapLastLapTime at -1).
    for n in 2..=5 {
        let l = &r.laps.iter().find(|l| l.lap.number == n).unwrap().lap;
        assert!(!l.valid, "lap {n}");
        assert!(
            l.invalid_reason.as_deref().unwrap().contains("meatball"),
            "lap {n}: {:?}",
            l.invalid_reason
        );
    }
    // the out-lap after the repair counts again
    assert!(lap(&r, 6, 146_105).lap.valid);
    // lap 6 includes the pit stop: out-lap with official time
    let pit_out = lap(&r, 6, 146_105);
    assert!(pit_out.lap.flags.out_lap);
    assert!(
        !pit_out
            .lap
            .invalid_reason
            .as_deref()
            .unwrap_or("")
            .contains("meatball")
    );
    // lap 5 ends in the pit lane
    let in_lap = r.laps.iter().find(|l| l.lap.number == 5).unwrap();
    assert!(in_lap.lap.flags.in_lap);
    // lap 2 had incidents and off-track moments
    let lap2 = r.laps.iter().find(|l| l.lap.number == 2).unwrap();
    assert!(!lap2.lap.valid);

    // telemetry content
    let t = &lap(&r, 7, 93_586).telemetry; // clean telemetry regardless of validity
    for ch in [
        names::TIME,
        names::DIST,
        names::SPEED,
        names::THROTTLE,
        names::BRAKE,
        names::STEER,
        names::GEAR,
        names::RPM,
        names::POS_X,
        names::POS_Y,
    ] {
        assert!(t.channel(ch).is_some(), "channel {ch} missing");
    }
    // 93.6 s at 60 Hz
    assert!((5600..5640).contains(&t.len()), "samples: {}", t.len());
}

#[test]
fn split_files_form_one_session() {
    let Some(all) = examples() else {
        return;
    };
    let files = files_matching(&all, "snetterton 300 2026-11-05 19-1");
    if files.len() < 2 {
        return;
    }
    let r = run(&files);
    print(&r);
    assert_eq!(
        r.sessions.len(),
        1,
        "both files belong to one iRacing session"
    );
    // lap 2 official time from the recording
    assert!(lap(&r, 2, 125_604).lap.valid);
    // lap 1 had an off-track moment with an incident
    assert!(!r.laps.iter().find(|l| l.lap.number == 1).unwrap().lap.valid);
}

#[test]
fn all_examples_decode_and_are_deterministic() {
    let Some(all) = examples() else {
        return;
    };
    let a = run(&all);
    let b = run(&all);
    print(&a);
    assert!(!a.laps.is_empty());
    let ids = |r: &Result| r.laps.iter().map(|l| l.lap.id).collect::<Vec<_>>();
    assert_eq!(ids(&a), ids(&b), "re-import yields the same ids");
    for l in &a.laps {
        let info = a
            .sessions
            .iter()
            .find(|s| s.id == l.lap.session_id)
            .and_then(|s| s.session_info.as_ref())
            .map(|i| i.to_string())
            .unwrap_or_default();
        assert!(!info.contains("IRating"), "no other drivers' data");
        assert!(!info.contains("ResultsPositions"));
    }
}

#[test]
fn winton_practice_has_no_pit_lane_laps() {
    let Some(all) = examples() else {
        return;
    };
    let files = files_matching(&all, "winton national 2026-11-10 17-00-40");
    if files.is_empty() {
        return;
    }
    let r = run(&files);
    print(&r);
    // The pit box is just before the line: "lap 0" (box to line) and the
    // seconds in the box after the reset at the end are not laps.
    let numbers: Vec<u32> = r.laps.iter().map(|l| l.lap.number).collect();
    assert_eq!(numbers, [1, 2, 3, 4, 5]);
    assert!(r.laps[0].lap.flags.out_lap);
    assert!(lap(&r, 4, 95_047).lap.valid);
}

/// Live telemetry has no GPS; the decoder reckons the position from
/// velocity and heading. It must match the GPS position of the same laps.
#[test]
fn dead_reckoning_matches_gps() {
    let Some(all) = examples() else {
        return;
    };
    let files = files_matching(&all, "winton national 2026-11-10 17-00-40");
    if files.is_empty() {
        return;
    }
    let gps = run(&files);
    let live = run_source(&mut LiveLike::new(IbtSource::open_many(&files).unwrap()));
    assert_eq!(gps.laps.len(), live.laps.len());
    let mut compared = 0;
    for (g, d) in gps.laps.iter().zip(&live.laps) {
        assert_eq!(g.lap.id, d.lap.id);
        assert!(
            d.telemetry.channel("iracing.lat").is_none(),
            "no GPS in live"
        );
        let xy = |l: &RecordedLap| -> [Vec<f64>; 2] {
            [names::POS_X, names::POS_Y].map(|n| match &l.telemetry.channel(n).unwrap().data {
                sy_model::ChannelData::F32(v) => v.iter().map(|&x| f64::from(x)).collect(),
                other => panic!("{n}: unexpected {other:?}"),
            })
        };
        let ([gx, gy], [dx, dy]) = (xy(g), xy(d));
        let mut max = 0.0f64;
        let mut known = 0;
        for i in 0..gx.len() {
            if dx[i].is_nan() {
                continue; // not anchored at the line yet (out-lap)
            }
            known += 1;
            max = max.max((gx[i] - dx[i]).hypot(gy[i] - dy[i]));
        }
        println!(
            "lap {}: {known}/{} samples, max {max:.1} m",
            g.lap.number,
            gx.len()
        );
        if !g.lap.flags.out_lap {
            assert_eq!(known, gx.len(), "lap {} fully positioned", g.lap.number);
        }
        if known > 0 {
            compared += 1;
            assert!(max < 5.0, "lap {}: {max:.1} m off", g.lap.number);
        }
    }
    assert!(compared >= 4);
}
