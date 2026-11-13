// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Le Mans Ultimate pipeline with real recordings (raw spool files of the
//! game).
//!
//! The recordings are personal data and not part of the repository. Put the
//! `.syraw` files into `<repo>/telemetry_examples/LMU/`; without them the
//! tests are skipped.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sy_capture::pipeline::{SpoolSource, recorder_config};
use sy_capture::{Decoder, RawSource, RecordedLap, Recorder, RecorderOutput};
use sy_model::channel::names;
use sy_model::{ChannelData, Session};
use sy_source_lmu::LmuDecoder;
use sy_source_lmu::records::{KIND_SCORING, KIND_SETUP, KIND_TELEMETRY, Scoring, Telemetry};

fn recordings() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../telemetry_examples/LMU");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "syraw"))
        .filter(|p| {
            std::env::var("LMU_RECORDING").map_or(true, |f| {
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

fn replay(path: &Path) -> Recorded {
    let mut source = SpoolSource::open(path).expect("open spool");
    let mut decoder = LmuDecoder::new();
    let mut recorder = Recorder::new(recorder_config(sy_model::GameId::Lmu));
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

/// (number, official time, valid, reason, out-lap, in-lap, incomplete)
type Summary = (u32, Option<u32>, bool, Option<String>, bool, bool, bool);

fn summary(l: &RecordedLap) -> Summary {
    let f = l.lap.flags;
    (
        l.lap.number,
        l.lap.lap_time_ms,
        l.lap.valid,
        l.lap.invalid_reason.clone(),
        f.out_lap,
        f.in_lap,
        f.incomplete,
    )
}

/// The replayed recording of a session on `track_key`.
fn session_on(track_key: &str) -> Option<Recorded> {
    recordings()
        .iter()
        .map(|p| replay(p))
        .find(|r| r.sessions.iter().any(|s| s.track_key == track_key))
}

fn reason(s: &str) -> Option<String> {
    Some(s.to_owned())
}

/// Distance of a lap: first and last sample, and every step forward and
/// small.
fn check_dist(l: &RecordedLap, from: std::ops::Range<f32>, to: std::ops::Range<f32>) {
    let d = f32s(l, names::DIST);
    assert!(
        from.contains(&d[0]),
        "lap {} starts at {}",
        l.lap.number,
        d[0]
    );
    let last = *d.last().unwrap();
    assert!(to.contains(&last), "lap {} ends at {last}", l.lap.number);
    for w in d.windows(2) {
        assert!(
            (-1.0..4.0).contains(&(w[1] - w[0])),
            "lap {}: {w:?}",
            l.lap.number
        );
    }
}

/// Practice at Monza (BMW M4 LMGT3, Nov 2026): out of the garage, one lap,
/// into the pits over the line. The game times neither lap (the out-lap
/// starts in the garage, the in-lap ends in the pit lane).
#[test]
fn monza_practice_out_and_in_lap() {
    let Some(r) = session_on("lmu:autodromo_nazionale_monza:5781") else {
        return;
    };
    assert_eq!(r.sessions.len(), 1);
    let s = &r.sessions[0];
    assert_eq!(s.track_name, "Autodromo Nazionale Monza (5.78 km)");
    assert_eq!(s.car_key, "lmu:bmw_m4_lmgt3:gt3");
    assert_eq!(s.car_name, "BMW M4 LMGT3");
    assert_eq!(s.session_type, sy_model::SessionType::Practice);
    let info = s.session_info.as_ref().unwrap();
    // the game reports no steering range for the car; the setup has it
    assert_eq!(info["steering_range_deg"], 516.0);
    assert_eq!(info["vehicle_class"], "GT3");
    let setup = s.setup.as_ref().unwrap();
    assert_eq!(setup["VM_STEER_LOCK"], "516 deg(19.7 )");
    let laps: Vec<Summary> = r.laps.iter().map(summary).collect();
    let not_timed = reason("not timed by the game");
    assert_eq!(
        laps,
        vec![
            (
                0,
                Some(160_470),
                false,
                not_timed.clone(),
                true,
                false,
                false
            ),
            (1, Some(136_720), false, not_timed, false, true, false),
        ]
    );
    // the garage is 60 m after the line
    check_dist(&r.laps[0], 55.0..65.0, 5770.0..5781.0);
    check_dist(&r.laps[1], 0.0..3.0, 5770.0..5782.0);
    // steering wheel angle from the setup's range: ±258°
    let steer = f32s(&r.laps[1], names::STEER);
    let max = steer.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(max <= 258f32.to_radians() + 1e-3, "{max}");
    let wear = f32s(&r.laps[1], "lmu.tyre_wear_fl");
    assert!(wear[0] >= 0.0 && wear.last() > wear.first(), "wear goes up");
    // the front right went flat in lap 1: the game reports it as worn out
    // from then on, the lap table shows the wear before and the puncture
    let c = sy_analysis::lap_conditions(sy_model::GameId::Lmu, &r.laps[1].telemetry);
    let flat = c.tyre_flat.unwrap();
    assert!(flat.fr && !flat.fl && !flat.rl && !flat.rr);
    let wear = c.tyre_wear.unwrap();
    assert!(wear.fr < 0.05 && wear.fl < 0.05, "{wear:?}");
    assert_eq!(c.weather.as_deref(), Some("Dry"));
    assert_eq!(c.fuel_unit.as_deref(), Some("l"));
    assert!(c.air_temp_c.is_some_and(|t| (16.0..18.0).contains(&t)));
}

/// Race on Monza's Curva Grande layout (Ford Mustang LMGT3, Nov 2026):
/// formation lap, rolling start, track-limits warnings in laps 1 and 3
/// (ours 0 and 2), a pit stop at the end of lap 2 (ours 1), back to the
/// garage after the flag.
#[test]
fn monza_curva_grande_race() {
    let Some(r) = session_on("lmu:monza_curva_grande_circuit:5745") else {
        return;
    };
    assert_eq!(r.sessions.len(), 1);
    let s = &r.sessions[0];
    assert_eq!(s.track_name, "Monza Curva Grande Circuit (5.75 km)");
    assert_eq!(s.car_key, "lmu:ford_mustang_lmgt3:gt3");
    assert_eq!(s.session_type, sy_model::SessionType::Race);
    let laps: Vec<Summary> = r.laps.iter().map(summary).collect();
    let limits = reason("track limits");
    assert_eq!(
        laps,
        vec![
            // from the green flag, 62 m before the line; the game doesn't
            // time it after the warning
            (0, Some(116_940), false, limits.clone(), false, false, false),
            // into the pits: the game times the in-lap in a race
            (1, Some(111_164), true, None, false, true, false),
            (2, Some(186_300), false, limits, true, false, false),
            (
                3,
                None,
                false,
                reason("incomplete, back to the garage"),
                false,
                false,
                true
            ),
        ]
    );
    assert_eq!(r.laps[1].lap.sector_times_ms, vec![27_573, 39_255, 44_336]);
    check_dist(&r.laps[0], -66.0..-60.0, 5740.0..5746.0);
    check_dist(&r.laps[1], 0.0..3.0, 5740.0..5747.0);
}

/// Practice at Long Beach (Ferrari 499P, Nov 2026): out of the garage,
/// back to the garage from the menu half-way round; out again, a clean
/// lap, into the pit box (before the line) and back to the garage.
#[test]
fn long_beach_hypercar_back_to_the_garage() {
    let Some(r) = session_on("lmu:grand_prix_of_long_beach:3187") else {
        return;
    };
    assert_eq!(r.sessions.len(), 1);
    let s = &r.sessions[0];
    assert_eq!(s.car_key, "lmu:ferrari_499p:hypercar");
    assert_eq!(s.car_name, "Ferrari 499P");
    let laps: Vec<Summary> = r.laps.iter().map(summary).collect();
    let garage = reason("incomplete, back to the garage");
    assert_eq!(
        laps,
        vec![
            (0, None, false, garage.clone(), true, false, true),
            (
                1,
                Some(123_920),
                false,
                reason("not timed by the game"),
                true,
                false,
                false
            ),
            (2, Some(77_307), true, None, false, false, false),
            (3, None, false, garage, false, true, true),
        ]
    );
    assert_eq!(r.laps[2].lap.sector_times_ms, vec![24_810, 27_211, 25_286]);
    // the garage is 172 m before the line; the first lap ends where the car
    // stood
    check_dist(&r.laps[0], -173.0..-172.0, 1615.0..1620.0);
    check_dist(&r.laps[2], 0.0..3.0, 3180.0..3188.0);
    // hybrid: virtual energy, battery and the motor
    let ve = f32s(&r.laps[2], "lmu.virtual_energy");
    assert!(
        ve.iter().all(|v| (0.9..=1.0).contains(v)),
        "{:?}",
        ve.first()
    );
    assert!(ve.last() < ve.first());
    let battery = f32s(&r.laps[2], "lmu.battery");
    assert!(battery.iter().all(|v| (0.0..=1.0).contains(v)));
    let regen = f32s(&r.laps[2], "lmu.regen");
    assert!(regen.iter().any(|w| *w > 100_000.0));
}

/// Qualifying at Road Atlanta in the rain (Lexus RCF LMGT3, Nov 2026):
/// out-lap, a lap the game stopped counting when the car slid off into the
/// grass (no track-limits warning), a clean lap, back to the garage.
#[test]
fn road_atlanta_rain_qualifying() {
    let Some(r) = session_on("lmu:michelin_raceway_road_atlanta:4083") else {
        return;
    };
    assert_eq!(r.sessions.len(), 1);
    let s = &r.sessions[0];
    assert_eq!(s.session_type, sy_model::SessionType::Qualifying);
    assert_eq!(s.car_key, "lmu:lexus_rcf_lmgt3:gt3");
    assert_eq!(
        s.conditions.weather.as_deref(),
        Some("rain 30 %, track wet 40 %")
    );
    // per lap (lap table): the rain eases, the track stays wet
    let weather: Vec<_> = r
        .laps
        .iter()
        .map(|l| sy_analysis::lap_conditions(sy_model::GameId::Lmu, &l.telemetry).weather)
        .collect();
    assert_eq!(weather[1].as_deref(), Some("Rain 30 %, track wet 60 %"));
    // on wet tyres: the name from the session info
    let names = sy_analysis::CompoundNames::from_session_info(
        sy_model::GameId::Lmu,
        s.session_info.as_ref(),
    );
    for l in &r.laps {
        let c = sy_analysis::lap_conditions_named(sy_model::GameId::Lmu, &l.telemetry, &names);
        assert_eq!(c.tyre_compound.map(|t| t.name).as_deref(), Some("Wet"));
    }
    assert_eq!(weather[3].as_deref(), Some("Rain 14 %, track wet 60 %"));
    let laps: Vec<Summary> = r.laps.iter().map(summary).collect();
    assert_eq!(
        laps,
        vec![
            (
                0,
                Some(141_320),
                false,
                reason("not timed by the game"),
                true,
                false,
                false
            ),
            (
                1,
                Some(118_100),
                false,
                reason("off track"),
                false,
                false,
                false
            ),
            (2, Some(110_901), true, None, false, false, false),
            (
                3,
                None,
                false,
                reason("incomplete, back to the garage"),
                false,
                false,
                true
            ),
        ]
    );
    assert_eq!(r.laps[2].lap.sector_times_ms, vec![13_144, 64_796, 32_961]);
    // the garage is 45 m before the line
    check_dist(&r.laps[0], -46.0..-44.0, 4075.0..4084.0);
    // rain easing off, the track wet; wet tarmac under the tyres
    let rain = f32s(&r.laps[2], "lmu.rain");
    assert!(
        rain.iter().all(|v| (0.14..0.31).contains(v)),
        "{:?}",
        rain.first()
    );
    let wet = f32s(&r.laps[2], "lmu.track_wetness");
    assert!(wet.iter().all(|v| (v - 0.6).abs() < 0.01));
    let surface = match &r.laps[2].telemetry.channel("lmu.surface_fl").unwrap().data {
        ChannelData::I32(v) => v.clone(),
        other => panic!("{other:?}"),
    };
    let wet_tarmac = surface.iter().filter(|s| **s == 1).count();
    assert!(
        wet_tarmac > surface.len() * 8 / 10,
        "{wet_tarmac} of {}",
        surface.len()
    );
}

/// Race at Sebring's school circuit (BMW M4 LMGT3, Nov 2026), restarted
/// from the menu just after the first lap ("Restart race"): two sessions.
/// The game times the first lap of a race from the green flag (rolling
/// start, 180 m before the line).
#[test]
fn sebring_race_restarted() {
    let Some(r) = session_on("lmu:sebring_school_circuit:3083") else {
        return;
    };
    assert_eq!(r.sessions.len(), 2);
    assert!(
        r.sessions
            .iter()
            .all(|s| s.session_type == sy_model::SessionType::Race)
    );
    assert_ne!(r.sessions[0].id, r.sessions[1].id);
    let of = |i: usize| -> Vec<Summary> {
        r.laps
            .iter()
            .filter(|l| l.lap.session_id == r.sessions[i].id)
            .map(summary)
            .collect()
    };
    // the line crossed a moment before the restart: no lap
    assert_eq!(
        of(0),
        vec![(0, Some(83_385), true, None, false, false, false)]
    );
    assert_eq!(
        of(1),
        vec![
            (0, Some(77_580), true, None, false, false, false),
            (1, Some(67_721), true, None, false, false, false),
            (
                2,
                None,
                false,
                reason("incomplete, back to the garage"),
                false,
                false,
                true
            ),
        ]
    );
    check_dist(&r.laps[0], -186.0..-180.0, 3075.0..3084.0);
    check_dist(&r.laps[1], -155.0..-148.0, 3075.0..3084.0);
}

/// Online practice at Spa (BMW M4 LMGT3, Nov 2026), joined 42 minutes
/// into the server's session: the session starts when the player joined.
#[test]
fn spa_online_practice() {
    let Some(r) = session_on("lmu:circuit_de_spa_francorchamps:6982") else {
        return;
    };
    assert_eq!(r.sessions.len(), 1);
    let s = &r.sessions[0];
    assert_eq!(s.session_type, sy_model::SessionType::Practice);
    assert_eq!(s.started_at.unix_timestamp(), 1_794_506_596);
    let info = s.session_info.as_ref().unwrap();
    assert_eq!(info["online"], true);
    let laps: Vec<Summary> = r.laps.iter().map(summary).collect();
    assert_eq!(
        laps,
        vec![
            (
                0,
                Some(157_210),
                false,
                reason("not timed by the game"),
                true,
                false,
                false
            ),
            (1, Some(143_982), true, None, false, false, false),
            (
                2,
                None,
                false,
                reason("incomplete, back to the garage"),
                false,
                false,
                true
            ),
        ]
    );
    assert_eq!(r.laps[1].lap.sector_times_ms, vec![38_316, 65_620, 40_046]);
}

fn records(path: &Path) -> Vec<sy_capture::RawRecord> {
    let mut source = SpoolSource::open(path).expect("open spool");
    let mut v = Vec::new();
    while let Some(rec) = source.next_record(Duration::ZERO).expect("read") {
        v.push(rec);
    }
    v
}

/// Debug: every change of the scoring fields that drive sessions and laps,
/// with the telemetry at that moment (run with --ignored --nocapture).
#[test]
#[ignore]
fn dump_scoring() {
    for path in recordings() {
        println!("=== {}", path.display());
        let mut telem: Option<Telemetry> = None;
        let mut last_key = String::new();
        let (mut nt, mut ns, mut nsetup) = (0, 0, 0);
        let mut first_t = None;
        let mut last_t = 0.0;
        for rec in records(&path) {
            match rec.kind {
                KIND_TELEMETRY => {
                    let t = Telemetry::parse(&rec.payload).unwrap();
                    nt += 1;
                    first_t.get_or_insert(t.elapsed_time);
                    last_t = t.elapsed_time;
                    if telem.as_ref().is_some_and(|o| o.lap_number != t.lap_number) {
                        println!(
                            "  telem lap {} -> {} at et {:.3} lap_start {:.3} sector {} invalid {} tl {}",
                            telem.as_ref().unwrap().lap_number,
                            t.lap_number,
                            t.elapsed_time,
                            t.lap_start_et,
                            t.current_sector,
                            t.lap_invalidated,
                            t.track_limits_steps
                        );
                    }
                    if telem.as_ref().is_some_and(|o| {
                        o.lap_invalidated != t.lap_invalidated
                            || o.track_limits_steps != t.track_limits_steps
                    }) {
                        println!(
                            "  telem et {:.3} lap {} invalid {} tl {}",
                            t.elapsed_time, t.lap_number, t.lap_invalidated, t.track_limits_steps
                        );
                    }
                    telem = Some(t);
                }
                KIND_SCORING => {
                    ns += 1;
                    let s = Scoring::parse(&rec.payload).unwrap();
                    let p = &s.player;
                    let key = format!(
                        "rain {:.2} wet {:.2}/{:.2}/{:.2} cloud {:.2} sess {} phase {} rt {} len {:.1} laps {} last {:.3} s1 {:.3} s2 {:.3} pits {} pit_state {} count {} ctrl {} nveh {}",
                        s.raining,
                        s.min_path_wetness,
                        s.avg_path_wetness,
                        s.max_path_wetness,
                        s.dark_cloud,
                        s.session,
                        s.game_phase,
                        s.in_realtime,
                        s.track_length,
                        p.total_laps,
                        p.last_lap_time,
                        p.last_sector1,
                        p.last_sector2,
                        p.in_pits,
                        p.pit_state,
                        p.count_lap_flag,
                        p.control,
                        s.num_vehicles,
                    );
                    if key != last_key {
                        let t = telem.as_ref();
                        println!(
                            "  et {:.3} [{}] {key} | dist {:.1} lap_start {:.3} | telem lap {:?} sector {:?} et {:?}",
                            s.current_et,
                            s.received_unix_ms,
                            p.lap_dist,
                            p.lap_start_et,
                            t.map(|t| t.lap_number),
                            t.map(|t| t.current_sector),
                            t.map(|t| t.elapsed_time),
                        );
                        last_key = key;
                    }
                }
                KIND_SETUP => {
                    nsetup += 1;
                    println!("  setup after {nt} telemetry, {ns} scoring (last et {last_t:.1})");
                }
                _ => {}
            }
        }
        println!(
            "  {nt} telemetry ({:.1} s, {:.1}/s), {ns} scoring, {nsetup} setups",
            last_t - first_t.unwrap_or(0.0),
            nt as f64 / (last_t - first_t.unwrap_or(0.0))
        );
    }
}

/// Debug: session info and the static fields (run with --ignored).
#[test]
#[ignore]
fn dump_info() {
    for path in recordings() {
        println!("=== {}", path.display());
        let recs = records(&path);
        let s = recs
            .iter()
            .find(|r| r.kind == KIND_SCORING)
            .map(|r| Scoring::parse(&r.payload).unwrap());
        let t = recs
            .iter()
            .find(|r| r.kind == KIND_TELEMETRY)
            .map(|r| Telemetry::parse(&r.payload).unwrap());
        if let Some(mut s) = s {
            s.player.driver_name = "<driver>".into();
            println!("{s:#?}");
        }
        if let Some(t) = t {
            println!("{t:#?}");
        }
    }
}

/// Debug: sessions and laps (run with --ignored).
#[test]
#[ignore]
fn dump_laps() {
    for path in recordings() {
        println!("=== {}", path.display());
        let r = replay(&path);
        for s in &r.sessions {
            println!(
                "  session {} {:?} {} | {} | {} | {}",
                s.started_at, s.session_type, s.track_key, s.track_name, s.car_key, s.car_name
            );
        }
        for l in &r.laps {
            println!(
                "  lap {} {:?} valid {} {:?} flags {:?} sectors {:?} samples {}",
                l.lap.number,
                l.lap.lap_time_ms,
                l.lap.valid,
                l.lap.invalid_reason,
                l.lap.flags,
                l.lap.sector_times_ms,
                l.telemetry.len()
            );
            let d = f32s(l, names::DIST);
            let jump = d
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0, f32::max);
            let back = d.windows(2).map(|w| w[0] - w[1]).fold(0.0, f32::max);
            println!(
                "      dist {:.1} .. {:.1}, largest step {jump:.2} m, largest step back {back:.2} m, steer {:?}",
                d[0],
                d[d.len() - 1],
                l.telemetry.channel(names::STEER).map(|_| {
                    f32s(l, names::STEER)
                        .iter()
                        .fold(0.0f32, |m, v| m.max(v.abs()))
                })
            );
        }
    }
}

/// Debug: every record in a time window (LMU_FROM/LMU_TO, run with
/// --ignored).
#[test]
#[ignore]
fn dump_window() {
    let env = |k: &str, d: f64| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    let (from, to) = (env("LMU_FROM", 0.0), env("LMU_TO", 1e9));
    let every = env("LMU_EVERY", 1.0) as usize;
    for path in recordings() {
        println!("=== {}", path.display());
        let mut n = 0usize;
        for rec in records(&path) {
            match rec.kind {
                KIND_TELEMETRY => {
                    let t = Telemetry::parse(&rec.payload).unwrap();
                    if !(from..=to).contains(&t.elapsed_time) {
                        continue;
                    }
                    n += 1;
                    if !n.is_multiple_of(every) {
                        continue;
                    }
                    println!(
                        "  T et {:.3} surf {:?} dt {:.4} lap {} start {:.3} sec {} pos [{:.1} {:.1} {:.1}] v {:.1} gear {} thr {:.2} brk {:.2} st {:.3} acc [{:.2} {:.2} {:.2}] rot [{:.3} {:.3} {:.3}] yaw {:.3} inv {} tl {} ve {:.4} bat {:.3}",
                        t.elapsed_time,
                        t.wheels.map(|w| w.surface_type),
                        t.delta_time,
                        t.lap_number,
                        t.lap_start_et,
                        t.current_sector,
                        t.pos[0],
                        t.pos[1],
                        t.pos[2],
                        t.speed(),
                        t.gear,
                        t.throttle,
                        t.brake,
                        t.steering,
                        t.local_accel[0],
                        t.local_accel[1],
                        t.local_accel[2],
                        t.local_rot[0],
                        t.local_rot[1],
                        t.local_rot[2],
                        (-t.ori[0][2]).atan2(-t.ori[2][2]),
                        t.lap_invalidated,
                        t.track_limits_steps,
                        t.virtual_energy,
                        t.battery,
                    );
                }
                KIND_SCORING => {
                    let s = Scoring::parse(&rec.payload).unwrap();
                    if !(from..=to).contains(&s.current_et) {
                        continue;
                    }
                    let p = &s.player;
                    println!(
                        "  S et {:.3} phase {} rt {} laps {} last {:.3} dist {:.1} start {:.3} pits {} ps {} count {} ctrl {}",
                        s.current_et,
                        s.game_phase,
                        s.in_realtime,
                        p.total_laps,
                        p.last_lap_time,
                        p.lap_dist,
                        p.lap_start_et,
                        p.in_pits,
                        p.pit_state,
                        p.count_lap_flag,
                        p.control
                    );
                }
                _ => {}
            }
        }
    }
}

/// Debug: signs of the map, steering, lateral g and yaw (run with
/// --ignored).
#[test]
#[ignore]
fn dump_signs() {
    for path in recordings() {
        println!("=== {}", path.display());
        let ts: Vec<Telemetry> = records(&path)
            .iter()
            .filter(|r| r.kind == KIND_TELEMETRY)
            .map(|r| Telemetry::parse(&r.payload).unwrap())
            .collect();
        // pairs 10 samples apart, driving
        let (mut n, mut steer_turn, mut glat_turn, mut yaw_turn, mut head_vel) = (0, 0, 0, 0, 0);
        let mut head_err = 0.0f64;
        let mut turning = 0.0f64;
        let mut yawrate_vs_dhead = 0;
        for w in ts.windows(21).step_by(5) {
            let (a, b, c) = (&w[0], &w[10], &w[20]);
            if b.speed() < 25.0 || c.elapsed_time - a.elapsed_time > 0.3 {
                continue;
            }
            // map: pos_x = x, pos_y = z
            let d1 = (b.pos[0] - a.pos[0], b.pos[2] - a.pos[2]);
            let d2 = (c.pos[0] - b.pos[0], c.pos[2] - b.pos[2]);
            let cross = d1.0 * d2.1 - d1.1 * d2.0; // > 0: counter-clockwise
            let ang = cross.atan2(d1.0 * d2.0 + d1.1 * d2.1);
            turning += ang / 2.0;
            if ang.abs() < 0.02 {
                continue;
            }
            n += 1;
            let right = ang < 0.0;
            let glat = -b.local_accel[0];
            if (b.steering > 0.0) == right {
                steer_turn += 1;
            }
            if (glat > 0.0) == right {
                glat_turn += 1;
            }
            if (b.local_rot[1] > 0.0) == right {
                yaw_turn += 1;
            }
            // heading of travel, clockwise from +pos_y
            let travel = d1.0.atan2(d1.1);
            let h = (-b.ori[0][2]).atan2(-b.ori[2][2]);
            let mut e = h - travel;
            while e > std::f64::consts::PI {
                e -= 2.0 * std::f64::consts::PI;
            }
            while e < -std::f64::consts::PI {
                e += 2.0 * std::f64::consts::PI;
            }
            head_err += e.abs();
            if e.abs() < 0.2 {
                head_vel += 1;
            }
            let hc = (-c.ori[0][2]).atan2(-c.ori[2][2]);
            let ha = (-a.ori[0][2]).atan2(-a.ori[2][2]);
            let mut dh = hc - ha;
            if dh > std::f64::consts::PI {
                dh -= 2.0 * std::f64::consts::PI;
            }
            if dh < -std::f64::consts::PI {
                dh += 2.0 * std::f64::consts::PI;
            }
            if (dh > 0.0) == (b.local_rot[1] > 0.0) {
                yawrate_vs_dhead += 1;
            }
        }
        println!(
            "  turning samples {n}: steer+ = right {steer_turn}, g_lat+ = right {glat_turn}, rot.y+ = right {yaw_turn}, heading ~ travel {head_vel} (mean err {:.3}), d(heading) same sign as rot.y {yawrate_vs_dhead}; total turning {:.1} rad (+ = counter-clockwise)",
            head_err / n.max(1) as f64,
            turning
        );
        let t = &ts[ts.len() / 2];
        println!(
            "  mid: steering range {} deg, brake temps {:?}, tyre temps K {:?}, pressures {:?}, wear {:?}, VE {}, battery {}, fuel {} / {}",
            t.steering_range_deg,
            t.wheels.map(|w| w.brake_temp_c.round()),
            t.wheels.map(|w| w.temperature_k.map(|k| k.round())),
            t.wheels.map(|w| w.pressure_kpa.round()),
            t.wheels.map(|w| (w.wear * 1000.0).round() / 1000.0),
            t.virtual_energy,
            t.battery,
            t.fuel,
            t.fuel_capacity
        );
        let ve: Vec<f32> = ts.iter().map(|t| t.virtual_energy).collect();
        let (lo, hi) = ve
            .iter()
            .fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
        let regen = ts.iter().map(|t| t.regen_kw).fold(f32::MIN, f32::max);
        let mt = ts.iter().map(|t| t.motor_torque).fold(f64::MIN, f64::max);
        println!(
            "  VE {lo}..{hi}, max regen {regen}, max motor torque {mt}, tc {:?} abs {:?} map {:?} gears {} compounds {} / {}",
            ts.iter().map(|t| t.tc).max(),
            ts.iter().map(|t| t.abs).max(),
            ts.iter().map(|t| t.motor_map).max(),
            t.max_gears,
            t.front_compound,
            t.rear_compound
        );
        let dts: Vec<f64> = ts
            .windows(2)
            .map(|w| w[1].elapsed_time - w[0].elapsed_time)
            .collect();
        let gaps = dts.iter().filter(|d| **d > 0.015).count();
        let maxgap = dts.iter().copied().fold(0.0, f64::max);
        for r in records(&path).iter().filter(|r| r.kind == KIND_SETUP) {
            let v: serde_json::Value = serde_json::from_slice(&r.payload).unwrap();
            println!("  setup steer lock {}", v["VM_STEER_LOCK"]["stringValue"]);
        }
        println!(
            "  {} steps, {gaps} longer than 15 ms, longest {maxgap:.3} s",
            dts.len()
        );
    }
}

/// Debug: the player's positions against the game's track map (REST
/// `/rest/watch/trackmap`, saved as `trackmap.json` at Monza; run with
/// --ignored).
#[test]
#[ignore]
fn dump_trackmap() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../telemetry_examples/LMU");
    let Ok(text) = std::fs::read(dir.join("trackmap.json")) else {
        return;
    };
    let map: Vec<serde_json::Value> = serde_json::from_slice(&text).unwrap();
    let pts: Vec<(f64, f64, f64, i64)> = map
        .iter()
        .map(|p| {
            (
                p["x"].as_f64().unwrap(),
                p["y"].as_f64().unwrap(),
                p["z"].as_f64().unwrap(),
                p["type"].as_i64().unwrap(),
            )
        })
        .collect();
    for path in recordings() {
        let recs = records(&path);
        let Some(s) = recs.iter().find(|r| r.kind == KIND_SCORING) else {
            continue;
        };
        let s = Scoring::parse(&s.payload).unwrap();
        println!("=== {} {} {}", path.display(), s.track_name, s.track_length);
        let mut ds = Vec::new();
        for (i, r) in recs.iter().filter(|r| r.kind == KIND_TELEMETRY).enumerate() {
            if !i.is_multiple_of(50) {
                continue;
            }
            let t = Telemetry::parse(&r.payload).unwrap();
            if t.speed() < 20.0 {
                continue;
            }
            let d = pts
                .iter()
                .filter(|p| p.3 == 0)
                .map(|p| {
                    ((p.0 - t.pos[0]).powi(2) + (p.1 - t.pos[1]).powi(2) + (p.2 - t.pos[2]).powi(2))
                        .sqrt()
                })
                .fold(f64::MAX, f64::min);
            ds.push(d);
        }
        ds.sort_by(f64::total_cmp);
        if ds.is_empty() {
            continue;
        }
        println!(
            "  {} samples: distance to the map's racing line median {:.1} m, 90 % {:.1} m, max {:.1} m",
            ds.len(),
            ds[ds.len() / 2],
            ds[ds.len() * 9 / 10],
            ds[ds.len() - 1]
        );
    }
}

/// Practice at Silverstone (BMW M4 LMGT3, Nov 2026): out-lap, a lap, back to
/// the garage. Going to the garage the game reports 0 K for the tyres: no
/// value, not −273 °C.
#[test]
fn silverstone_practice_lap_conditions() {
    let Some(r) = session_on("lmu:silverstone_grand_prix_circuit_wec:5869") else {
        return;
    };
    let c: Vec<_> = r
        .laps
        .iter()
        .map(|l| sy_analysis::lap_conditions(sy_model::GameId::Lmu, &l.telemetry))
        .collect();
    assert_eq!(c.len(), 3);
    for (i, c) in c.iter().enumerate() {
        assert_eq!(c.weather.as_deref(), Some("Dry"), "lap {i}");
        assert!(
            c.air_temp_c.is_some_and(|t| (14.0..17.0).contains(&t)),
            "lap {i}: {c:?}"
        );
        assert!(
            c.track_temp_c.is_some_and(|t| (22.0..24.0).contains(&t)),
            "lap {i}: {c:?}"
        );
        let temp = c.tyre_temp_c.unwrap();
        assert!((40.0..70.0).contains(&temp.fl), "lap {i}: {temp:?}");
        assert!(
            c.tyre_wear.is_some() && c.tyre_flat.is_none(),
            "lap {i}: {c:?}"
        );
    }
    let used = c[1].fuel_used.unwrap();
    assert!((3.0..3.7).contains(&used), "{used} l");
    let garage = f32s(&r.laps[2], "lmu.tyre_temp_fl");
    assert!(
        garage.iter().all(|t| t.is_nan() || *t > 0.0),
        "no 0 K readings"
    );
}
