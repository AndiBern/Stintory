// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! F1 25 pipeline with real recordings (raw spool files of the game).
//!
//! The recordings are personal data and not part of the repository. Put the
//! `.syraw` files into `<repo>/telemetry_examples/F125/`; without them the
//! tests are skipped.

use std::path::PathBuf;
use std::time::Duration;

use sy_capture::pipeline::recorder_config;
use sy_capture::pipeline::{SpoolChain, SpoolSource};
use sy_capture::{Decoder, RawSource, RecordedLap, Recorder, RecorderOutput};
use sy_model::channel::names;
use sy_model::{ChannelData, Session};
use sy_source_f1::F1Decoder;

fn recordings() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../telemetry_examples/F125");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "syraw"))
        // the debug dumps can be limited to some files
        .filter(|p| {
            std::env::var("F1_RECORDING").map_or(true, |f| {
                f.split(',').any(|f| p.to_string_lossy().contains(f.trim()))
            })
        })
        .collect();
    files.sort();
    files
}

struct Recorded {
    sessions: Vec<Session>,
    laps: Vec<RecordedLap>,
}

fn replay(path: &std::path::Path) -> Recorded {
    replay_all(&[path.to_path_buf()])
}

fn replay_all(paths: &[PathBuf]) -> Recorded {
    let mut source = SpoolChain::open(paths).expect("open spools");
    let mut decoder = F1Decoder::new();
    let mut recorder = Recorder::new(recorder_config(sy_model::GameId::F1_25));
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
    Recorded { sessions, laps }
}

fn f32s(lap: &RecordedLap, name: &str) -> Vec<f32> {
    match &lap.telemetry.channel(name).expect(name).data {
        ChannelData::F32(v) => v.clone(),
        other => panic!("{name}: {other:?}"),
    }
}

/// (number, official time, valid, out-lap, flashback, incomplete)
type Summary = (u32, Option<u32>, bool, bool, bool, bool);

fn summary(l: &RecordedLap) -> Summary {
    let f = l.lap.flags;
    (
        l.lap.number,
        l.lap.lap_time_ms,
        l.lap.valid,
        f.out_lap,
        f.flashback,
        f.incomplete,
    )
}

/// Time trial at Catalunya, F1 (McLaren), 2026 format, Nov 2026. The
/// validity matches the game's own lap history (valid bits) and what was
/// driven: track limits in laps 2, 3, 8; flashbacks in 6 and 9 (time trial
/// invalidates those); lap 1 carries the invalidation from the out-lap.
#[test]
fn catalunya_time_trial() {
    let Some(r) = recordings().iter().map(|p| replay(p)).find(|r| {
        r.sessions
            .iter()
            .any(|s| s.started_at.unix_timestamp() == 1_794_416_003)
    }) else {
        return;
    };
    let s = &r.sessions[0];
    assert_eq!(s.track_key, "f1_25:4");
    assert_eq!(s.track_name, "Catalunya");
    assert_eq!(s.car_key, "f1_25:8/0");
    assert_eq!(s.car_name, "McLaren (F1 2025)");
    assert_eq!(s.session_type, sy_model::SessionType::TimeTrial);
    let laps: Vec<Summary> = r.laps.iter().map(summary).collect();
    assert_eq!(
        laps,
        vec![
            (0, Some(laps[0].1.unwrap()), true, true, false, false),
            (1, Some(77_429), false, false, false, false),
            (2, Some(77_853), false, false, false, false),
            (3, Some(76_439), false, false, false, false),
            (4, Some(75_689), true, false, false, false),
            (5, Some(75_165), true, false, false, false),
            (6, Some(83_368), false, false, true, false),
            (7, Some(86_986), false, false, false, false),
            (8, Some(76_847), false, false, false, false),
            (9, Some(75_675), false, false, true, false),
            (10, Some(75_909), true, false, false, false),
            (11, None, false, false, false, true),
        ]
    );
    // the out-lap starts at the pit exit, 36 m after the line
    let d = f32s(&r.laps[0], names::DIST);
    assert!((30.0..40.0).contains(&d[0]), "{}", d[0]);
    assert!(*d.last().unwrap() > 4600.0);
    assert_eq!(r.laps[4].lap.sector_times_ms, vec![22_376, 30_674, 22_639]);
}

/// The recording whose first packet arrived at `unix_s` (session time 0).
fn recording(unix_s: i64) -> Option<PathBuf> {
    recordings().into_iter().find(|p| {
        let mut src = SpoolSource::open(p).unwrap();
        let rec = src.next_record(Duration::ZERO).unwrap().unwrap();
        i64::from_le_bytes(rec.payload[..8].try_into().unwrap()) / 1000 == unix_s
    })
}

fn check(r: &Recorded, track: &str, car_key: &str, car: &str, laps: &[Summary]) {
    let s = &r.sessions[0];
    assert_eq!(r.sessions.len(), 1);
    assert_eq!(
        (
            s.track_name.as_str(),
            s.car_key.as_str(),
            s.car_name.as_str()
        ),
        (track, car_key, car)
    );
    let mut got: Vec<Summary> = r.laps.iter().map(summary).collect();
    // the out-lap's time is our measurement
    if let Some(out) = got.first_mut().filter(|l| l.3) {
        out.1 = None;
    }
    assert_eq!(got, laps, "{track} {car}");
}

/// F1 26 car (Mercedes) at Suzuka, time trial.
#[test]
fn suzuka_f1_26() {
    let Some(path) = recording(1_794_417_637) else {
        return;
    };
    check(
        &replay(&path),
        "Suzuka",
        "f1_25:476/13",
        "Mercedes (F1 26)",
        &[
            (0, None, true, true, false, false),
            (1, Some(93_898), false, false, false, false),
            (2, Some(96_908), true, false, false, false),
            (3, None, false, false, false, true),
        ],
    );
}

/// F2 2026 (Invicta) at Jeddah, time trial, paused for two minutes during
/// the first timed lap. The old app ended the recording during the pause
/// (two files); live it stays one recording now, as replayed here. Session
/// time stands still while paused, so the lap time (117.390 s, the game's)
/// doesn't include the pause.
#[test]
fn jeddah_f2_2026_with_a_pause() {
    let (Some(a), Some(b)) = (recording(1_794_418_219), recording(1_794_418_566)) else {
        return;
    };
    check(
        &replay_all(&[a, b]),
        "Jeddah",
        "f1_25:499/2",
        "Invicta (F2 2026)",
        &[
            (0, None, true, true, false, false),
            (1, Some(117_390), false, false, false, false),
            (2, None, false, false, false, true),
        ],
    );
}

/// F2 2025 (Campos) at Austria, time trial.
#[test]
fn austria_f2_2025() {
    let Some(path) = recording(1_794_418_650) else {
        return;
    };
    check(
        &replay(&path),
        "Austria",
        "f1_25:466/2",
        "Campos (F2 2025)",
        &[
            (0, None, true, true, false, false),
            (1, Some(76_117), false, false, false, false),
            (2, None, false, false, false, true),
        ],
    );
}

/// F2 2024 (Rodin) at Austria reversed, time trial: the start is only
/// 185 m before the line.
#[test]
fn austria_reverse_f2_2024() {
    let Some(path) = recording(1_794_418_795) else {
        return;
    };
    let r = replay(&path);
    check(
        &r,
        "Austria (Reverse)",
        "f1_25:160/2",
        "Rodin Motorsport (F2 2024)",
        &[
            (0, None, true, true, false, false),
            (1, Some(78_746), true, false, false, false),
            (2, None, false, false, false, true),
        ],
    );
    let d = f32s(&r.laps[0], names::DIST);
    assert!((4100.0..4200.0).contains(&d[0]), "{}", d[0]);
}

/// Five-lap race in Brazil with the F1 World car: standing start (lap 1
/// starts on the grid, 42 m before the line), a flashback in lap 1, and
/// the last lap, where the game's lap number doesn't go up at the flag.
#[test]
fn brazil_race_f1_world() {
    let Some(path) = recording(1_794_418_970) else {
        return;
    };
    let r = replay(&path);
    check(
        &r,
        "Brazil",
        "f1_25:41/8",
        "F1 Generic (F1 World)",
        &[
            (1, Some(82_107), true, false, true, false),
            (2, Some(74_983), true, false, false, false),
            (3, Some(74_063), true, false, false, false),
            (4, Some(73_981), true, false, false, false),
            (5, Some(74_520), true, false, false, false),
        ],
    );
    assert_eq!(r.sessions[0].session_type, sy_model::SessionType::Race);
    let d = f32s(&r.laps[0], names::DIST);
    assert!((-45.0..-40.0).contains(&d[0]), "{}", d[0]);
}

/// Time trial in Austria with the F1 26 Ferrari (Nov 2026), checking the
/// invalidation reasons: running wide in 1 and 2 (2 was a cut over grass,
/// the game calls it running wide), a flashback in 3, running wide before
/// the line in 4 ("this and next lap", so 5 too), a flashback in lap 7
/// back across the line into lap 6 (the game re-times lap 6 and
/// invalidates it), lap 7 clean, then a restart of lap 8 from the menu:
/// the game puts the car before the line and keeps the lap number. Times
/// and validity match the game's lap history.
#[test]
fn austria_f1_26_track_limits_and_restart() {
    let Some(path) = recording(1_794_479_460) else {
        return;
    };
    let r = replay(&path);
    check(
        &r,
        "Austria",
        "f1_25:477/13",
        "Ferrari (F1 26)",
        &[
            (0, None, true, true, false, false),
            (1, Some(69_539), false, false, false, false),
            (2, Some(69_494), false, false, false, false),
            (3, Some(69_515), false, false, true, false),
            (4, Some(68_967), false, false, false, false),
            (5, Some(69_837), false, false, false, false),
            (6, Some(67_898), false, false, true, false),
            (7, Some(67_467), true, false, false, false),
            (8, None, false, false, false, true),
            (8, Some(67_185), true, false, false, false),
            (9, None, false, false, false, true),
        ],
    );
    let reasons: Vec<_> = r
        .laps
        .iter()
        .map(|l| l.lap.invalid_reason.as_deref())
        .collect();
    assert_eq!(
        reasons[1..9],
        [
            Some("running wide"),
            Some("running wide"),
            Some("flashback used"),
            Some("running wide"),
            Some("running wide"),
            Some("flashback used"),
            None,
            Some("incomplete, lap restarted"),
        ]
    );
    // the restarted lap starts at the line, without the run-up
    let d = f32s(&r.laps[9], names::DIST);
    assert!(d[0] < 5.0, "{}", d[0]);
}

/// Silverstone: the game sat in the menu before the time trial (session
/// time 0) and was never driven. No session.
#[test]
fn silverstone_menu_only() {
    let Some(path) = recording(1_794_480_342) else {
        return;
    };
    let r = replay(&path);
    assert!(r.sessions.is_empty());
    assert!(r.laps.is_empty());
}

/// Spa with the Alpine (F1 2025), time trial: the game sent UDP format 2025
/// at first (most of it didn't arrive), then the player restarted the lap,
/// switched to format 2026 in the game's settings, restarted again and
/// drove a lap. The session goes on across the switch.
#[test]
fn spa_format_switch() {
    let Some(path) = recording(1_794_480_939) else {
        return;
    };
    let r = replay(&path);
    check(
        &r,
        "Spa",
        "f1_25:5/0",
        "Alpine (F1 2025)",
        &[
            (2, None, false, false, false, true),
            (2, None, false, false, false, true),
            (2, None, false, false, false, true),
            (2, Some(108_998), true, false, false, false),
            (3, None, false, false, false, true),
        ],
    );
    assert_eq!(
        r.sessions[0].session_info.as_ref().unwrap()["udp_format"],
        2026
    );
    assert!(r.laps[3].telemetry.channel("f1.active_aero_mode").is_some());
}

/// Hungaroring with the Red Bull (F1 2025), time trial in UDP format 2025.
#[test]
fn hungaroring_format_2025() {
    let Some(path) = recording(1_794_481_478) else {
        return;
    };
    let r = replay(&path);
    check(
        &r,
        "Hungaroring",
        "f1_25:2/0",
        "Red Bull Racing (F1 2025)",
        &[
            (0, None, true, true, false, false),
            (1, Some(86_914), true, false, false, false),
            (2, None, false, false, false, true),
        ],
    );
    assert_eq!(
        r.sessions[0].session_info.as_ref().unwrap()["udp_format"],
        2025
    );
    assert_eq!(r.laps[1].lap.sector_times_ms, vec![28_596, 33_607, 24_711]);
}

/// Five-lap race in Shanghai with the F1 26 Cadillac: standing start (the
/// grid is after the line here), a pit stop at the end of lap 2, a pause
/// in lap 4, the flag at the end of lap 5.
#[test]
fn shanghai_race_pit_stop() {
    let Some(path) = recording(1_794_481_898) else {
        return;
    };
    let r = replay(&path);
    check(
        &r,
        "Shanghai",
        "f1_25:486/13",
        "Cadillac (F1 26)",
        &[
            (1, Some(117_543), true, false, false, false),
            (2, Some(128_419), true, false, false, false),
            (3, Some(124_881), true, true, false, false),
            (4, Some(99_336), true, false, false, false),
            (5, Some(101_772), true, false, false, false),
        ],
    );
    assert!(r.laps[1].lap.flags.in_lap);
    assert!(!r.laps[2].lap.flags.in_lap && !r.laps[3].lap.flags.out_lap);
}

/// Every recording: timed laps start at the line, the map is not mirrored
/// (steering left turns the path counter-clockwise) and lateral g is
/// positive in right-hand turns.
#[test]
fn axes_and_distances() {
    for path in recordings() {
        let r = replay(&path);
        let Some(session) = r.sessions.first() else {
            continue; // never drove (menus only)
        };
        let race = session.session_type == sy_model::SessionType::Race;
        // a race's lap 1 starts on the grid
        for l in r.laps.iter().filter(|l| !(race && l.lap.number == 1)) {
            let d = f32s(l, names::DIST);
            assert!(
                d.iter().all(|x| *x >= -1.0),
                "{}: lap {} has negative distances",
                path.display(),
                l.lap.number
            );
        }
        let (mut ccw_left, mut cw_left, mut g_right, mut g_left) = (0, 0, 0, 0);
        for l in r.laps.iter().filter(|l| !l.lap.flags.incomplete) {
            let (x, y) = (f32s(l, names::POS_X), f32s(l, names::POS_Y));
            let steer = f32s(l, names::STEER_NORM);
            let glat = f32s(l, names::G_LAT);
            for i in 2..x.len() {
                let cross = (x[i - 1] - x[i - 2]) * (y[i] - y[i - 1])
                    - (y[i - 1] - y[i - 2]) * (x[i] - x[i - 1]);
                if steer[i].abs() > 0.2 && cross.abs() > 1e-3 {
                    if (cross > 0.0) == (steer[i] < 0.0) {
                        ccw_left += 1;
                    } else {
                        cw_left += 1;
                    }
                    if (glat[i] > 0.0) == (steer[i] > 0.0) {
                        g_right += 1;
                    } else {
                        g_left += 1;
                    }
                }
            }
        }
        if ccw_left + cw_left < 500 {
            continue; // no complete laps (e.g. half of a recording)
        }
        assert!(ccw_left > 20 * cw_left, "{}: mirrored map", path.display());
        assert!(g_right > 20 * g_left, "{}: lateral g sign", path.display());
    }
}

/// Debug dump of the lap data (run with --ignored --nocapture).
#[test]
#[ignore]
fn dump_lap_data() {
    use sy_source_f1::packets::{Event, Header, LapData, SessionHistory, id};
    for path in recordings() {
        let mut source = SpoolSource::open(&path).unwrap();
        let mut last: Option<LapData> = None;
        let mut n = 0u64;
        while let Some(rec) = source.next_record(Duration::ZERO).unwrap() {
            let p = &rec.payload[8..];
            let Ok(h) = Header::parse(p) else { continue };
            n += 1;
            match h.packet_id {
                id::LAP_DATA => {
                    let l = LapData::parse(&h, p);
                    let changed = last.is_none_or(|x| {
                        x.lap != l.lap
                            || x.pit_status != l.pit_status
                            || x.driver_status != l.driver_status
                            || x.lap_invalid != l.lap_invalid
                            || (x.lap_distance_m < 0.0) != (l.lap_distance_m < 0.0)
                    });
                    if changed {
                        println!(
                            "t={:8.3} frame={} lap={} dist={:8.1} pit={} drv={} invalid={} last={} cur={} warn={} cc={} res={}",
                            h.session_time,
                            h.frame,
                            l.lap,
                            l.lap_distance_m,
                            l.pit_status,
                            l.driver_status,
                            l.lap_invalid,
                            l.last_lap_ms,
                            l.current_lap_ms,
                            l.warnings,
                            l.corner_cutting_warnings,
                            l.result_status
                        );
                    }
                    last = Some(l);
                }
                id::EVENT => println!("t={:8.3} EVENT {:?}", h.session_time, Event::parse(p)),
                id::SESSION_HISTORY if n % 2000 < 40 => {
                    let s = SessionHistory::parse(p);
                    let v: Vec<_> = s
                        .laps
                        .iter()
                        .map(|l| (l.lap_time_ms, l.valid_flags))
                        .collect();
                    println!("t={:8.3} HISTORY {:?}", h.session_time, v);
                }
                _ => {}
            }
        }
    }
}

/// Debug: signs of the map, steering and lateral g (run with --ignored).
#[test]
#[ignore]
fn dump_signs() {
    for path in recordings() {
        let r = replay(&path);
        for l in r.laps.iter().filter(|l| l.lap.valid) {
            let (x, y) = (f32s(l, names::POS_X), f32s(l, names::POS_Y));
            let steer = f32s(l, names::STEER_NORM);
            let glat = f32s(l, names::G_LAT);
            let yaw = f32s(l, names::YAW);
            let (mut agree, mut disagree, mut g_same, mut g_opp) = (0, 0, 0, 0);
            let (mut yaw_ok, mut yaw_bad) = (0, 0);
            for i in 2..x.len() {
                let (dx1, dy1) = (x[i - 1] - x[i - 2], y[i - 1] - y[i - 2]);
                let (dx2, dy2) = (x[i] - x[i - 1], y[i] - y[i - 1]);
                // > 0: turning counter-clockwise (left on a right-handed map)
                let cross = dx1 * dy2 - dy1 * dx2;
                if steer[i].abs() > 0.1 && cross.abs() > 1e-3 {
                    if (cross > 0.0) == (steer[i] < 0.0) {
                        agree += 1
                    } else {
                        disagree += 1
                    }
                    if (glat[i] > 0.0) == (steer[i] > 0.0) {
                        g_same += 1
                    } else {
                        g_opp += 1
                    }
                }
                // heading of travel vs yaw
                let h = dx2.atan2(dy2); // from +y towards +x
                let d = (h - yaw[i]).sin().abs();
                if d < 0.2 { yaw_ok += 1 } else { yaw_bad += 1 }
            }
            println!(
                "lap {}: left steer turns CCW {agree} vs {disagree}; g_lat same sign as steer {g_same} vs {g_opp}; yaw≈atan2(dx,dz) {yaw_ok} vs {yaw_bad}",
                l.lap.number
            );
        }
    }
}

/// Debug: sessions and laps of every recording (run with --ignored).
#[test]
#[ignore]
fn dump_laps() {
    for path in recordings() {
        let r = replay(&path);
        println!("== {}", path.file_name().unwrap().to_string_lossy());
        for s in &r.sessions {
            let info = s.session_info.as_ref().unwrap();
            println!(
                "session {} {:?} {} / {} ({}) {} → {:?} fmt={} formula={}",
                s.id,
                s.session_type,
                s.track_name,
                s.car_name,
                s.car_key,
                s.started_at,
                s.ended_at.map(|e| e.time()),
                info["udp_format"],
                info["formula"]
            );
        }
        for l in &r.laps {
            let d = f32s(l, names::DIST);
            println!(
                "  lap {:>2} {:>9?} valid={} {:?} out={} in={} inc={} fb={} n={} dist {:.0}..{:.0} t0={:?} {:?}",
                l.lap.number,
                l.lap.lap_time_ms,
                l.lap.valid,
                l.lap.invalid_reason,
                l.lap.flags.out_lap,
                l.lap.flags.in_lap,
                l.lap.flags.incomplete,
                l.lap.flags.flashback,
                l.telemetry.len(),
                d.first().unwrap(),
                d.last().unwrap(),
                l.lap.start_time_s,
                l.lap.sector_times_ms,
            );
        }
    }
}

/// Debug: events, pause flags and the last lap data of each file (run with
/// --ignored).
#[test]
#[ignore]
fn dump_ends() {
    use sy_source_f1::packets::{Event, Header, LapData, Session, SessionHistory, id};
    for path in recordings() {
        println!("== {}", path.file_name().unwrap().to_string_lossy());
        let mut source = SpoolSource::open(&path).unwrap();
        let (mut first, mut last_t, mut last_paused, mut count) = (None, 0.0f32, None, 0u64);
        let mut tail: Vec<String> = Vec::new();
        let mut last_hist = None;
        while let Some(rec) = source.next_record(Duration::ZERO).unwrap() {
            let ts = i64::from_le_bytes(rec.payload[..8].try_into().unwrap());
            let p = &rec.payload[8..];
            let Ok(h) = Header::parse(p) else { continue };
            first.get_or_insert((ts, h.session_time));
            last_t = h.session_time;
            count += 1;
            match h.packet_id {
                id::EVENT => println!("  wall={ts} t={:8.3} {:?}", h.session_time, Event::parse(p)),
                id::SESSION => {
                    let s = Session::parse(&h, p);
                    if last_paused != Some(s.paused) {
                        println!("  wall={ts} t={:8.3} paused={}", h.session_time, s.paused);
                        last_paused = Some(s.paused);
                    }
                }
                id::LAP_DATA => {
                    let l = LapData::parse(&h, p);
                    tail.push(format!(
                        "  wall={ts} t={:8.3} lap={} dist={:7.1} last={} cur={} drv={} res={} pit={}",
                        h.session_time, l.lap, l.lap_distance_m, l.last_lap_ms, l.current_lap_ms,
                        l.driver_status, l.result_status, l.pit_status
                    ));
                }
                id::SESSION_HISTORY => {
                    let s = SessionHistory::parse(p);
                    last_hist = Some(
                        s.laps
                            .iter()
                            .map(|l| (l.lap_time_ms, l.valid_flags))
                            .collect::<Vec<_>>(),
                    );
                }
                _ => {}
            }
        }
        println!("  first {:?} last t={last_t} packets={count}", first);
        // lap data where it changes in the last part
        let mut prev = String::new();
        for line in tail.iter().rev().take(4000).rev() {
            let key: String = line
                .split(" lap=")
                .nth(1)
                .unwrap_or_default()
                .split(" dist")
                .next()
                .unwrap_or_default()
                .to_owned()
                + line
                    .split(" last=")
                    .nth(1)
                    .unwrap_or_default()
                    .split(' ')
                    .next()
                    .unwrap_or_default()
                + line.split(" res=").nth(1).unwrap_or_default();
            if key != prev {
                println!("{line}");
                prev = key;
            }
        }
        println!("  {}", tail.last().cloned().unwrap_or_default());
        println!("  history {:?}", last_hist);
    }
}

/// Debug: lap data twice a second between two session times
/// (F1_FROM/F1_TO, run with --ignored).
#[test]
#[ignore]
fn dump_window() {
    use sy_source_f1::packets::{CarMotion, Event, Header, LapData, Session, id};
    let env = |k: &str, d: f32| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    let (from, to) = (env("F1_FROM", 0.0), env("F1_TO", f32::MAX));
    for path in recordings() {
        println!("== {}", path.file_name().unwrap().to_string_lossy());
        let mut source = SpoolSource::open(&path).unwrap();
        let mut next = 0.0f32;
        let mut pos = (0.0, 0.0);
        while let Some(rec) = source.next_record(Duration::ZERO).unwrap() {
            let ts = i64::from_le_bytes(rec.payload[..8].try_into().unwrap());
            let p = &rec.payload[8..];
            let Ok(h) = Header::parse(p) else { continue };
            if h.session_time < from || h.session_time > to {
                continue;
            }
            match h.packet_id {
                id::MOTION => {
                    let m = CarMotion::parse(&h, p);
                    pos = (m.position[0], m.position[2]);
                }
                id::EVENT => println!("  wall={ts} t={:8.3} {:?}", h.session_time, Event::parse(p)),
                id::SESSION => {
                    let s = Session::parse(&h, p);
                    if s.paused {
                        println!("  wall={ts} t={:8.3} paused", h.session_time);
                    }
                }
                id::LAP_DATA if h.session_time >= next || (h.session_time - next).abs() > 5.0 => {
                    next = h.session_time + 0.5;
                    let l = LapData::parse(&h, p);
                    println!(
                        "  wall={ts} t={:8.3} fmt={:?} lap={} dist={:7.1} cur={} last={} pit={} drv={} inv={} res={} pos=({:.0},{:.0})",
                        h.session_time,
                        h.format,
                        l.lap,
                        l.lap_distance_m,
                        l.current_lap_ms,
                        l.last_lap_ms,
                        l.pit_status,
                        l.driver_status,
                        l.lap_invalid,
                        l.result_status,
                        pos.0,
                        pos.1
                    );
                }
                _ => {}
            }
        }
    }
}

/// Debug: packets per 10 s of wall time, by format and packet id (run with
/// --ignored).
#[test]
#[ignore]
fn dump_rates() {
    use sy_source_f1::packets::Header;
    for path in recordings() {
        println!("== {}", path.file_name().unwrap().to_string_lossy());
        let mut source = SpoolSource::open(&path).unwrap();
        let mut bucket: Option<i64> = None;
        let mut counts = std::collections::BTreeMap::new();
        let mut t = (0.0f32, 0.0f32);
        let flush = |b: i64, counts: &std::collections::BTreeMap<(u16, u8), u32>, t: (f32, f32)| {
            println!("  +{:4}s t={:7.1}..{:7.1} {:?}", b * 10, t.0, t.1, counts);
        };
        let mut first = None;
        while let Some(rec) = source.next_record(Duration::ZERO).unwrap() {
            let ts = i64::from_le_bytes(rec.payload[..8].try_into().unwrap());
            let first = *first.get_or_insert(ts);
            let b = (ts - first) / 10_000;
            let p = &rec.payload[8..];
            let fmt = u16::from_le_bytes([p[0], p[1]]);
            let (id, time) = match Header::parse(p) {
                Ok(h) => (h.packet_id, h.session_time),
                Err(_) => (255, 0.0),
            };
            if bucket != Some(b) {
                if let Some(old) = bucket {
                    flush(old, &counts, t);
                }
                counts.clear();
                bucket = Some(b);
                t = (time, time);
            }
            t.1 = time;
            *counts.entry((fmt, id)).or_insert(0u32) += 1;
        }
        if let Some(b) = bucket {
            flush(b, &counts, t);
        }
    }
}
