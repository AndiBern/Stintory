// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Le Mans Ultimate through the whole pipeline (decoder + recorder), with
//! synthetic records (`sy_source_lmu::sim`). Real recordings:
//! `lmu_recordings.rs`.

use std::collections::VecDeque;
use std::time::Duration;

use sy_capture::pipeline::{RunOptions, Sink, SpoolSource, run};
use sy_capture::spool::SpoolWriter;
use sy_capture::{
    Decoder, RawRecord, RawSource, RecordedLap, RecorderOutput, SourceError, SourceEvent,
};
use sy_model::channel::names;
use sy_model::{ChannelData, GameId, SessionType};
use sy_source_lmu::LmuDecoder;
use sy_source_lmu::sim::SimLmu;
use sy_store::StoreError;

struct Script(VecDeque<RawRecord>);

impl Script {
    fn new(records: Vec<RawRecord>) -> Self {
        Self(records.into())
    }
}

impl RawSource for Script {
    fn game(&self) -> GameId {
        GameId::Lmu
    }

    fn next_record(&mut self, _: Duration) -> Result<Option<RawRecord>, SourceError> {
        Ok(self.0.pop_front())
    }
}

#[derive(Default)]
struct Collect(Vec<RecorderOutput>);

impl Sink for Collect {
    fn apply(&mut self, output: RecorderOutput) -> Result<(), StoreError> {
        self.0.push(output);
        Ok(())
    }
}

fn opts<'a>(spool: Option<&'a mut SpoolWriter>) -> RunOptions<'a> {
    RunOptions {
        spool,
        should_stop: &|| false,
        idle_done: None,
        progress: None,
    }
}

fn record(records: Vec<RawRecord>) -> Collect {
    let mut sink = Collect::default();
    run(&mut Script::new(records), &mut sink, opts(None)).unwrap();
    sink
}

fn laps(out: &Collect) -> Vec<&RecordedLap> {
    out.0
        .iter()
        .filter_map(|o| match o {
            RecorderOutput::Lap(l) => Some(l.as_ref()),
            _ => None,
        })
        .collect()
}

fn sessions(out: &Collect) -> Vec<&sy_model::Session> {
    out.0
        .iter()
        .filter_map(|o| match o {
            RecorderOutput::SessionStarted(s) => Some(s),
            _ => None,
        })
        .collect()
}

fn ms(s: f64) -> u32 {
    (s * 1000.0).round() as u32
}

/// Out-lap from the pits, then laps as the closures say.
#[test]
fn practice_laps_times_and_validity() {
    let mut g = SimLmu::new();
    let mut rec = g.drive_lap(1.0); // out-lap (lap 0)
    let mut times = Vec::new();

    rec.extend(g.drive_lap(1.0)); // lap 1: clean
    times.push((g.last_lap_time, g.last_sectors));

    rec.extend(g.drive(10.0)); // lap 2: track limits
    g.track_limits_steps += 2;
    g.lap_invalidated = true;
    rec.extend(g.drive_lap(1.0));
    times.push((g.last_lap_time, g.last_sectors));

    g.untimed_next_lap = true; // lap 3: the game counts no time
    rec.extend(g.drive_lap(1.0));

    rec.extend(g.drive_lap(1.0)); // lap 4: clean
    times.push((g.last_lap_time, g.last_sectors));
    rec.extend(g.drive(5.0)); // into lap 5, cut off by the end

    let out = record(rec);
    let s = sessions(&out);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].game, GameId::Lmu);
    assert_eq!(s[0].track_key, "lmu:sim_oval:2000");
    assert_eq!(s[0].track_name, "Sim Oval (2.00 km)");
    assert_eq!(s[0].car_key, "lmu:porsche_963:hypercar");
    assert_eq!(s[0].car_name, "Porsche 963");
    assert_eq!(s[0].session_type, SessionType::Practice);
    assert_eq!(s[0].game_version.as_deref(), Some("1200"));
    assert_eq!(s[0].conditions.air_temp_c, Some(21.0));
    // started at receive time minus elapsed time (the sim's epoch)
    assert_eq!(
        s[0].started_at.unix_timestamp(),
        g.epoch_ms.div_euclid(1000)
    );
    let info = s[0].session_info.as_ref().unwrap();
    assert_eq!(info["vehicle_class"], "Hypercar");
    assert_eq!(info["championship"], "WEC 2025");
    assert!(info.get("server_name").is_none());

    let l = laps(&out);
    let numbers: Vec<u32> = l.iter().map(|l| l.lap.number).collect();
    assert_eq!(numbers, [0, 1, 2, 3, 4, 5]);

    assert!(l[0].lap.flags.out_lap);
    assert!(!l[0].lap.valid);

    let check_time = |lap: &RecordedLap, (t, (s1, s12)): (f64, (f64, f64))| {
        assert_eq!(lap.lap.lap_time_ms, Some(ms(t)), "lap {}", lap.lap.number);
        assert_eq!(
            lap.lap.sector_times_ms,
            vec![ms(s1), ms(s12) - ms(s1), ms(t) - ms(s12)]
        );
    };
    check_time(l[1], times[0]);
    assert!(l[1].lap.valid);
    assert!(!l[1].lap.flags.out_lap);

    check_time(l[2], times[1]);
    assert!(!l[2].lap.valid);
    assert_eq!(l[2].lap.invalid_reason.as_deref(), Some("track limits"));

    assert!(!l[3].lap.valid);
    assert_eq!(
        l[3].lap.invalid_reason.as_deref(),
        Some("not timed by the game")
    );

    check_time(l[4], times[2]);
    assert!(l[4].lap.valid);

    assert!(l[5].lap.flags.incomplete);

    // the channels the milestone is about
    let tel = &l[1].telemetry;
    for name in [
        names::SPEED,
        names::STEER,
        names::ABS_ACTIVE,
        names::TC_ACTIVE,
        "lmu.virtual_energy",
        "lmu.tc_level",
        "lmu.abs_level",
        "lmu.tyre_temp_fl",
    ] {
        assert!(tel.channel(name).is_some(), "{name}");
    }
    match &tel.channel("lmu.virtual_energy").unwrap().data {
        ChannelData::F32(v) => assert!(v.iter().all(|x| (0.0..=1.0).contains(x))),
        other => panic!("{other:?}"),
    }
    match &tel.channel(names::TC_ACTIVE).unwrap().data {
        ChannelData::Bool(v) => assert!(v.iter().any(|x| *x) && v.iter().any(|x| !*x)),
        other => panic!("{other:?}"),
    }
    match &tel.channel("lmu.tyre_temp_fl").unwrap().data {
        ChannelData::F32(v) => assert!((v[0] - 85.0).abs() < 0.01, "{}", v[0]),
        other => panic!("{other:?}"),
    }
    // the lap's distance goes from the line to the track length
    match &tel.channel(names::DIST).unwrap().data {
        ChannelData::F32(d) => {
            assert!(d[0] < 2.0, "{}", d[0]);
            let last = *d.last().unwrap();
            assert!((last - 2000.0).abs() < 2.0, "{last}");
            assert!(d.windows(2).all(|w| w[1] >= w[0]), "monotonic");
        }
        other => panic!("{other:?}"),
    }
}

/// The 100 Hz distance from 5 Hz scoring stays close to the truth.
#[test]
fn lap_distance_is_interpolated() {
    let mut g = SimLmu::new();
    let mut rec = g.drive_lap(1.0);
    rec.extend(g.drive_lap(1.0));
    rec.extend(g.drive_lap(1.0));
    let mut d = LmuDecoder::new();
    let mut frames = Vec::new();
    for r in &rec {
        for e in d.decode(r).unwrap() {
            if let SourceEvent::Frame(f) = e {
                frames.push(f);
            }
        }
    }
    assert!(frames.len() > 3000, "{}", frames.len());
    let mut worst = 0.0f64;
    for f in &frames {
        let (_, truth) = g
            .truth
            .iter()
            .find(|(t, _)| (*t - f.session_time).abs() < 1e-9)
            .expect("a sample at that time");
        let mut err = (f64::from(f.lap_dist) - truth).abs();
        if err > 1000.0 {
            err = (err - 2000.0).abs(); // at the line
        }
        worst = worst.max(err);
    }
    // speed varies ±40 % along the lap; the scoring corrects every 0.2 s
    assert!(worst < 1.5, "worst distance error {worst} m");
}

/// The scoring may report the finished lap a little before the telemetry
/// crosses the line: every lap still gets its own time. (Short laps: the
/// decoder forgets laps waiting for a time after 30 s.)
#[test]
fn scoring_before_the_line() {
    let mut g = SimLmu::new();
    g.track_length = 1000.0;
    g.dist = 700.0;
    g.pit_exit = 850.0;
    let mut rec = g.drive_lap(1.0);
    let mut times = Vec::new();
    for _ in 0..3 {
        let mut lap = g.drive_lap(1.0);
        times.push(g.last_lap_time);
        // move the scoring record that first shows the new lap count in
        // front of the first telemetry sample of the new lap
        use sy_source_lmu::records::{KIND_SCORING, KIND_TELEMETRY, Scoring, Telemetry};
        let first_scoring = lap
            .iter()
            .position(|r| {
                r.kind == KIND_SCORING
                    && Scoring::parse(&r.payload).unwrap().player.total_laps == g.total_laps
            })
            .unwrap();
        let first_telemetry = lap
            .iter()
            .position(|r| {
                r.kind == KIND_TELEMETRY
                    && Telemetry::parse(&r.payload).unwrap().lap_number == g.lap
            })
            .unwrap();
        if first_telemetry < first_scoring {
            let scoring = lap.remove(first_scoring);
            lap.insert(first_telemetry, scoring);
        }
        rec.extend(lap);
    }
    let out = record(rec);
    let l = laps(&out);
    let got: Vec<Option<u32>> = l[1..4].iter().map(|l| l.lap.lap_time_ms).collect();
    let want: Vec<Option<u32>> = times.iter().map(|&t| Some(ms(t))).collect();
    assert_eq!(got, want);
    assert!(l[1..4].iter().all(|l| l.lap.valid));
}

/// Back to the garage mid-lap: the lap is incomplete, the next one is an
/// out-lap again.
#[test]
fn leaving_the_car_aborts_the_lap() {
    let mut g = SimLmu::new();
    let mut rec = g.drive_lap(1.0);
    rec.extend(g.drive_lap(1.0));
    rec.extend(g.drive(15.0));
    g.in_realtime = false;
    rec.extend(g.drive(5.0));
    // back out of the pits
    g.in_realtime = true;
    g.in_pits = true;
    g.dist = 1700.0;
    g.untimed_next_lap = true;
    rec.extend(g.drive_lap(1.0));
    rec.extend(g.drive_lap(1.0));

    let out = record(rec);
    assert_eq!(sessions(&out).len(), 1);
    let l = laps(&out);
    let summary: Vec<(u32, bool, bool, bool)> = l
        .iter()
        .map(|l| {
            (
                l.lap.number,
                l.lap.valid,
                l.lap.flags.out_lap,
                l.lap.flags.incomplete,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (0, false, true, false),
            (1, true, false, false),
            (2, false, false, true),
            (2, false, true, false),
            (3, true, false, false),
            (4, false, false, true),
        ]
    );
    assert_eq!(
        l[2].lap.invalid_reason.as_deref(),
        Some("incomplete, left the car")
    );
}

/// Back to the garage from the menu: the game puts the car into the garage
/// and counts the lap as completed, before the scoring says the player
/// left the car. On track the lap is incomplete; in the pit lane (the pit
/// box before the line) it is an incomplete in-lap.
#[test]
fn back_to_the_garage_from_the_menu() {
    let mut g = SimLmu::new();
    let mut rec = g.drive_lap(1.0); // out-lap
    rec.extend(g.drive(15.0)); // lap 1, half-way
    rec.extend(g.back_to_garage());
    rec.extend(g.drive(3.0));
    // out again: lap 2 is an out-lap, then lap 3 into the pit box
    g.in_realtime = true;
    g.untimed_next_lap = true;
    rec.extend(g.drive_lap(1.0));
    rec.extend(g.drive(15.0));
    g.in_pits = true;
    rec.extend(g.drive(5.0));
    rec.extend(g.back_to_garage());
    rec.extend(g.drive(3.0));

    let out = record(rec);
    let l = laps(&out);
    let summary: Vec<(u32, bool, bool, bool, Option<&str>)> = l
        .iter()
        .map(|l| {
            (
                l.lap.number,
                l.lap.flags.out_lap,
                l.lap.flags.in_lap,
                l.lap.flags.incomplete,
                l.lap.invalid_reason.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (0, true, false, false, Some("not timed by the game")),
            (
                1,
                false,
                false,
                true,
                Some("incomplete, back to the garage")
            ),
            (2, true, false, false, Some("not timed by the game")),
            (3, false, true, true, Some("incomplete, back to the garage")),
        ]
    );
    // the lap ends where the car was, not in the garage
    match &l[1].telemetry.channel(names::DIST).unwrap().data {
        ChannelData::F32(d) => {
            let last = *d.last().unwrap();
            assert!((600.0..1000.0).contains(&last), "{last}");
        }
        other => panic!("{other:?}"),
    }
}

/// Races: nothing is recorded before the start; the first lap starts at
/// the green flag, before the line (rolling start), and counts until the
/// line after a whole lap. A lap the game doesn't time after a
/// track-limits warning is invalid for track limits.
#[test]
fn race_start_and_track_limits() {
    let mut g = SimLmu::race();
    let mut rec = Vec::new();
    while g.dist < 1940.0 {
        rec.extend(g.drive(0.1)); // formation lap
    }
    let green = g.et;
    g.green_flag();
    rec.extend(g.drive_lap(1.0)); // lap 0
    let line = g.lap_start_et;
    rec.extend(g.drive(10.0)); // lap 1: a warning, no time
    g.track_limits_steps += 1;
    g.untimed_next_lap = true;
    rec.extend(g.drive_lap(1.0));
    rec.extend(g.drive_lap(1.0)); // lap 2: clean
    let lap2 = g.last_lap_time;

    let out = record(rec);
    assert_eq!(sessions(&out)[0].session_type, SessionType::Race);
    let l = laps(&out);
    let numbers: Vec<u32> = l.iter().map(|l| l.lap.number).collect();
    assert_eq!(numbers, [0, 1, 2, 3]);
    let first = &l[0];
    assert!(!first.lap.flags.out_lap && !first.lap.flags.incomplete);
    match &first.telemetry.channel(names::DIST).unwrap().data {
        ChannelData::F32(d) => {
            assert!((-62.0..-55.0).contains(&d[0]), "{}", d[0]);
            let last = *d.last().unwrap();
            assert!((last - 2000.0).abs() < 2.0, "{last}");
            assert!(d.windows(2).all(|w| w[1] >= w[0]), "monotonic");
        }
        other => panic!("{other:?}"),
    }
    // from the green flag to the line (the sim's lap 0 isn't timed by the
    // game: our measured time)
    let t = f64::from(first.lap.lap_time_ms.unwrap()) / 1000.0;
    assert!((t - (line - green)).abs() < 0.05, "{t} vs {}", line - green);
    assert_eq!(
        l[1].lap.invalid_reason.as_deref(),
        Some("track limits"),
        "lap 1"
    );
    assert_eq!(l[2].lap.lap_time_ms, Some(ms(lap2)));
    assert!(l[2].lap.valid);
}

/// Off the track the game stops counting the lap (`mCountLapFlag` 1)
/// without a track-limits warning; when it counts it again (2) the lap is
/// timed.
#[test]
fn off_track_without_a_warning() {
    let mut g = SimLmu::new();
    let mut rec = g.drive_lap(1.0); // out-lap
    rec.extend(g.drive(10.0)); // lap 1: off, forgiven
    g.count_lap_flag = 1;
    rec.extend(g.drive(2.0));
    g.count_lap_flag = 2;
    rec.extend(g.drive_lap(1.0));
    rec.extend(g.drive(10.0)); // lap 2: off, not counted
    g.count_lap_flag = 1;
    g.untimed_next_lap = true;
    rec.extend(g.drive_lap(0.0));
    g.count_lap_flag = 2;
    rec.extend(g.drive_lap(1.0)); // lap 3

    let out = record(rec);
    let l = laps(&out);
    let summary: Vec<(u32, bool, Option<&str>)> = l
        .iter()
        .map(|l| (l.lap.number, l.lap.valid, l.lap.invalid_reason.as_deref()))
        .collect();
    assert_eq!(
        summary,
        [
            (0, false, Some("not timed by the game")),
            (1, true, None),
            (2, false, Some("off track")),
            (3, true, None),
            (4, false, Some("incomplete")),
        ]
    );
}

/// Online the server's session may have run for long before the player
/// joined: the session starts when they joined. The key stays the
/// server's session, so reconnecting keeps it.
#[test]
fn online_session_starts_when_joining() {
    let mut g = SimLmu::new();
    g.game_mode = 2;
    g.et = 2500.0;
    g.lap_start_et = 2500.0;
    let mut rec = g.drive_lap(1.0);
    rec.extend(g.drive_lap(1.0));
    let out = record(rec);
    let s = sessions(&out);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].started_at.unix_timestamp(), g.epoch_ms / 1000 + 2500);
    assert_eq!(s[0].session_info.as_ref().unwrap()["online"], true);
}

/// Restarting the session (elapsed time back to the start) and the next
/// session give new sessions.
#[test]
fn restarts_and_new_sessions() {
    let mut g = SimLmu::new();
    let mut rec = g.drive_lap(1.0);
    rec.extend(g.drive_lap(1.0));
    // restart: time from the beginning, back in the pits
    let epoch = g.epoch_ms;
    g = SimLmu::new();
    g.epoch_ms = epoch + 120_000;
    rec.extend(g.drive_lap(1.0));
    rec.extend(g.drive_lap(1.0));
    // qualifying
    let mut q = SimLmu::new();
    q.session = 5;
    q.epoch_ms = epoch + 600_000;
    rec.extend(q.drive_lap(1.0));
    rec.extend(q.drive_lap(1.0));

    let out = record(rec);
    let s = sessions(&out);
    assert_eq!(s.len(), 3);
    assert_ne!(s[0].id, s[1].id);
    assert_eq!(s[2].session_type, SessionType::Qualifying);
    let ended = out
        .0
        .iter()
        .filter(|o| matches!(o, RecorderOutput::SessionEnded(_)))
        .count();
    assert_eq!(ended, 3);
    // out-lap, a lap and the start of the next one in each
    assert_eq!(laps(&out).len(), 9);
}

/// The setup from the REST API becomes the session's setup.
#[test]
fn setup_is_stored() {
    let mut g = SimLmu::new();
    let mut rec = vec![SimLmu::setup(r#"{"wing": 3}"#)];
    rec.extend(g.drive_lap(1.0));
    rec.push(SimLmu::setup(r#"{"wing": 4}"#));
    rec.extend(g.drive_lap(1.0));
    let out = record(rec);
    assert_eq!(
        sessions(&out)[0].setup,
        Some(serde_json::json!({"wing": 3}))
    );
    let last = out
        .0
        .iter()
        .rev()
        .find_map(|o| match o {
            RecorderOutput::SessionUpdated(s) | RecorderOutput::SessionEnded(s) => Some(s),
            _ => None,
        })
        .unwrap();
    assert_eq!(last.setup, Some(serde_json::json!({"wing": 4})));
}

/// Spooled data holds the player only, and replaying the spool gives the
/// same laps.
#[test]
fn spool_replay_is_identical_and_player_only() {
    let mut g = SimLmu::new();
    let mut rec = g.drive_lap(1.0);
    rec.extend(g.drive_lap(1.0));
    rec.extend(g.drive_lap(1.0));
    for r in &rec {
        let text = String::from_utf8_lossy(&r.payload);
        assert!(!text.contains("Some Server"));
        // the player's own name is only in the player's scoring
        assert!(text.matches("Sim Driver").count() <= 1);
    }

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lmu.syraw");
    let mut writer = SpoolWriter::create(&path, GameId::Lmu).unwrap();
    let mut live = Collect::default();
    run(&mut Script::new(rec), &mut live, opts(Some(&mut writer))).unwrap();
    writer.finish().unwrap();

    let mut replay = Collect::default();
    let mut src = SpoolSource::open(&path).unwrap();
    run(&mut src, &mut replay, opts(None)).unwrap();

    let key = |c: &Collect| {
        laps(c)
            .iter()
            .map(|l| {
                (
                    l.lap.id,
                    l.lap.lap_time_ms,
                    l.lap.valid,
                    l.telemetry.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(key(&live).len(), 4);
    assert!(key(&live) == key(&replay));
    assert_eq!(sessions(&live)[0].id, sessions(&replay)[0].id);
}
