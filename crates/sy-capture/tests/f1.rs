// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! F1 25 through the whole pipeline (decoder + recorder), with synthetic
//! packets of both UDP formats (`sy_source_f1::sim`). Real recordings will
//! replace these once we have them (docs/games/f1.md).

use std::collections::VecDeque;
use std::time::Duration;

use sy_capture::pipeline::{RunOptions, Sink, run};
use sy_capture::{RawRecord, RawSource, RecordedLap, RecorderOutput, SourceError};
use sy_model::{GameId, SessionType};
use sy_source_f1::decoder::packet_record;
use sy_source_f1::filter::player_only;
use sy_source_f1::packets::Format;
use sy_source_f1::sim::SimGame;
use sy_store::StoreError;

/// Packets as the UDP source would deliver them.
#[derive(Default)]
struct Script {
    records: VecDeque<RawRecord>,
}

impl Script {
    fn send(&mut self, packet: &[u8]) {
        let filtered = player_only(packet).expect("a packet Stintory keeps");
        self.records
            .push_back(packet_record(1_794_000_000_000, &filtered));
    }

    /// Drives `seconds` at 20 Hz, sending every frame's packets.
    fn drive(&mut self, g: &mut SimGame, seconds: f32) {
        let steps = (seconds * 20.0).round() as usize;
        for _ in 0..steps {
            g.advance(0.05);
            for p in g.frame() {
                self.send(&p);
            }
        }
    }

    /// Drives until the lap number changes, then on for `extra` seconds
    /// (the session history follows the line).
    fn drive_lap(&mut self, g: &mut SimGame) {
        let lap = g.lap;
        while g.lap == lap {
            self.drive(g, 0.05);
        }
        self.send(&g.session_history());
        self.drive(g, 1.0);
    }
}

impl RawSource for Script {
    fn game(&self) -> GameId {
        GameId::F1_25
    }

    fn next_record(&mut self, _: Duration) -> Result<Option<RawRecord>, SourceError> {
        Ok(self.records.pop_front())
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

fn record(script: Script) -> Collect {
    let mut source = script;
    let mut sink = Collect::default();
    run(
        &mut source,
        &mut sink,
        RunOptions {
            spool: None,
            should_stop: &|| false,
            idle_done: None,
            progress: None,
        },
    )
    .unwrap();
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

/// (number, time, valid, out-lap, in-lap, incomplete, flashback)
type Summary = (u32, Option<u32>, bool, bool, bool, bool, bool);

fn summary(l: &RecordedLap) -> Summary {
    let f = l.lap.flags;
    (
        l.lap.number,
        l.lap.lap_time_ms,
        l.lap.valid,
        f.out_lap,
        f.in_lap,
        f.incomplete,
        f.flashback,
    )
}

#[test]
fn time_trial_validity_and_flashbacks() {
    for format in [Format::F2025, Format::F2026] {
        let mut g = SimGame::new(format);
        let mut s = Script::default();
        s.send(&g.session());
        s.send(&g.participants());
        s.send(&g.event(b"SSTA", &[]));

        // out-lap: leaves the pit lane at 600 m
        g.dist = 600.0;
        g.pit = 2;
        g.driver_status = 3;
        s.drive(&mut g, 1.0);
        g.pit = 0;
        s.drive_lap(&mut g);
        g.driver_status = 1;

        // lap 2: clean
        s.drive_lap(&mut g);

        // lap 3: track limits
        s.drive(&mut g, 5.0);
        g.invalid = true;
        s.drive_lap(&mut g);

        // lap 4: flashback mid-lap, stays valid
        s.drive(&mut g, 6.0);
        let back = g.clone();
        s.drive(&mut g, 4.0);
        s.send(&g.flashback(back.time));
        g = back;
        s.drive_lap(&mut g);

        // lap 5: invalidated, then a flashback to before it undoes that
        s.drive(&mut g, 6.0);
        let back = g.clone();
        s.drive(&mut g, 2.0);
        g.invalid = true;
        s.drive(&mut g, 2.0);
        s.send(&g.flashback(back.time));
        g = back;
        s.drive_lap(&mut g);

        // lap 6 is cut off by the end of the session
        s.drive(&mut g, 5.0);
        s.send(&g.event(b"SEND", &[]));
        s.drive(&mut g, 1.0);

        let out = record(s);
        let RecorderOutput::SessionStarted(session) = &out.0[0] else {
            panic!("{:?}", out.0[0]);
        };
        assert_eq!(session.session_type, SessionType::TimeTrial);
        assert_eq!(session.track_key, "f1_25:10");
        assert_eq!(session.car_key, "f1_25:4/0");
        let lap_ms = 20_000; // 1000 m at 50 m/s
        assert_eq!(
            laps(&out).iter().map(|l| summary(l)).collect::<Vec<_>>(),
            vec![
                (1, Some(lap_ms), true, true, false, false, false),
                (2, Some(lap_ms), true, false, false, false, false),
                (3, Some(lap_ms), false, false, false, false, false),
                (4, Some(lap_ms), true, false, false, false, true),
                (5, Some(lap_ms), true, false, false, false, true),
                (6, None, false, false, false, true, false),
            ],
            "format {format}"
        );
        let l = laps(&out);
        assert_eq!(
            l[2].lap.invalid_reason.as_deref(),
            Some("invalidated by the game")
        );
        assert_eq!(l[1].lap.sector_times_ms, vec![6_666, 6_666, 6_668]);
        // the flashback removed the rewound samples: one pass over the lap
        let n = l[3].telemetry.len();
        assert!((395..=405).contains(&n), "{n} samples");
        assert!(matches!(
            out.0.last(),
            Some(RecorderOutput::SessionEnded(_))
        ));
    }
}

#[test]
fn race_laps_and_the_pit_stop() {
    let mut g = SimGame::new(Format::F2025);
    g.session_type = 15;
    let mut s = Script::default();
    s.send(&g.session());
    s.send(&g.participants());
    // standing start just behind the line
    g.dist = -30.0;
    g.driver_status = 4;
    s.drive(&mut g, 0.5);
    s.drive_lap(&mut g);
    // lap 2 ends in the pit lane, lap 3 starts there
    while g.dist < 900.0 {
        s.drive(&mut g, 0.05);
    }
    g.pit = 1;
    s.drive_lap(&mut g);
    s.drive(&mut g, 2.0);
    g.pit = 0;
    s.drive_lap(&mut g);
    // the last lap: at the flag the game keeps the lap number and only
    // reports the lap time; then the cool-down lap
    while g.dist < 990.0 {
        s.drive(&mut g, 0.05);
    }
    s.send(&g.event(b"CHQF", &[]));
    g.dist = 0.0;
    g.last_lap_ms = 19_876;
    s.drive(&mut g, 3.0);
    s.send(&g.event(b"SEND", &[]));

    let out = record(s);
    let l = laps(&out);
    assert_eq!(
        l.iter().map(|l| summary(l)).collect::<Vec<_>>(),
        vec![
            // the game's time (the sim always reports 20 s)
            (1, Some(20_000), true, false, false, false, false),
            (2, Some(20_000), true, false, true, false, false),
            (3, Some(20_000), true, true, false, false, false),
            (4, Some(19_876), true, false, false, false, false),
            // the cool-down after the flag
            (5, None, false, false, false, true, false),
        ]
    );
}

#[test]
fn spectating_records_nothing() {
    let mut g = SimGame::new(Format::F2026);
    let mut s = Script::default();
    let mut session = g.session();
    session[44] = 1; // isSpectating
    s.send(&session);
    s.send(&g.participants());
    s.drive_lap(&mut g);
    s.drive_lap(&mut g);
    let out = record(s);
    assert!(laps(&out).is_empty());
}

/// The live path: UDP socket → spool → store; replaying the spool (crash
/// recovery) gives the same laps.
#[test]
fn udp_to_store_and_replay() {
    use std::net::UdpSocket;
    use std::sync::{Arc, Mutex};

    use sy_capture::pipeline::{SharedStore, SpoolSource};
    use sy_capture::spool::SpoolWriter;
    use sy_source_f1::{UdpConfig, UdpSource};
    use sy_store::Store;

    let mut source = UdpSource::bind(&UdpConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        forward: Vec::new(),
    })
    .unwrap();
    let to = source.local_addr().unwrap();
    let game = std::thread::spawn(move || {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut g = SimGame::new(Format::F2026);
        // record the packets unfiltered: the source filters
        let mut packets = vec![g.session(), g.participants()];
        for _ in 0..3 {
            let lap = g.lap;
            while g.lap == lap {
                g.advance(0.05);
                packets.extend(g.frame());
            }
        }
        packets.push(g.event(b"SEND", &[]));
        for (i, p) in packets.iter().enumerate() {
            socket.send_to(p, to).unwrap();
            if i % 50 == 0 {
                // don't overrun the receive buffer
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let store = SharedStore(Arc::new(Mutex::new(Store::open(dir.path()).unwrap())));
    let spool_path = dir.path().join("f1.syraw");
    let mut writer = SpoolWriter::create(&spool_path, GameId::F1_25).unwrap();
    let mut sink = store.clone();
    let mut idle = |d: Duration| d > Duration::from_millis(500);
    let stats = run(
        &mut source,
        &mut sink,
        RunOptions {
            spool: Some(&mut writer),
            should_stop: &|| false,
            idle_done: Some(&mut idle),
            progress: None,
        },
    )
    .unwrap();
    writer.finish().unwrap();
    game.join().unwrap();
    assert_eq!(stats.sessions, 1);
    let sessions = store.with(|s| s.list_sessions()).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].game, GameId::F1_25);
    // laps 1-2 complete (lap 1 starts at the line), lap 3 a sliver
    assert_eq!(sessions[0].best_lap_ms, Some(20_000));
    let stored = store.with(|s| s.laps(sessions[0].id)).unwrap();

    let dir2 = tempfile::tempdir().unwrap();
    let mut replayed = SharedStore(Arc::new(Mutex::new(Store::open(dir2.path()).unwrap())));
    let mut spool = SpoolSource::open(&spool_path).unwrap();
    run(
        &mut spool,
        &mut replayed,
        RunOptions {
            spool: None,
            should_stop: &|| false,
            idle_done: None,
            progress: None,
        },
    )
    .unwrap();
    let again = replayed.with(|s| s.laps(sessions[0].id)).unwrap();
    assert_eq!(
        stored
            .iter()
            .map(|l| (l.id, l.lap_time_ms, l.valid))
            .collect::<Vec<_>>(),
        again
            .iter()
            .map(|l| (l.id, l.lap_time_ms, l.valid))
            .collect::<Vec<_>>(),
        "same ids, times and validity"
    );
}

/// A time trial that starts shortly before the line: the bit before the
/// line is an out-lap, not part of lap 1.
#[test]
fn time_trial_start_just_before_the_line() {
    let mut g = SimGame::new(Format::F2026);
    let mut s = Script::default();
    s.send(&g.session());
    s.send(&g.participants());
    g.dist = -185.0;
    s.drive_lap(&mut g); // still lap 1 for the game: crosses at 0 m
    let lap1 = g.lap;
    while g.lap == lap1 {
        s.drive(&mut g, 0.05);
    }
    s.send(&g.event(b"SEND", &[]));
    let out = record(s);
    let l = laps(&out);
    assert_eq!(l[0].lap.number, 0);
    assert!(l[0].lap.flags.out_lap);
    assert_eq!(l[1].lap.number, 1);
    assert!(!l[1].lap.flags.out_lap);
}

/// The game's penalties tell why a lap is invalid; a flashback takes a
/// penalty back.
#[test]
fn invalidation_reasons_from_penalties() {
    let mut g = SimGame::new(Format::F2026);
    let mut s = Script::default();
    s.send(&g.session());
    s.send(&g.participants());
    s.drive_lap(&mut g); // lap 1: clean

    // lap 2: running wide
    s.drive(&mut g, 5.0);
    g.invalid = true;
    s.send(&g.penalty(10, 26));
    s.drive_lap(&mut g);

    // lap 3: corner cutting at the end, which also costs lap 4
    s.drive(&mut g, 18.0);
    g.invalid = true;
    s.send(&g.penalty(11, 25));
    s.drive_lap(&mut g);
    g.invalid = true;
    s.drive_lap(&mut g);

    // lap 5: running wide, then a flashback to before it: the game takes
    // the penalty back, but in time trial the flashback invalidates the lap
    s.drive(&mut g, 5.0);
    let back = g.clone();
    s.drive(&mut g, 2.0);
    s.send(&g.penalty(10, 26));
    s.send(&g.flashback(back.time));
    g = back;
    g.invalid = true;
    s.send(&g.penalty(10, 31));
    s.drive_lap(&mut g);
    s.send(&g.event(b"SEND", &[]));

    let out = record(s);
    let reasons: Vec<(u32, Option<String>)> = laps(&out)
        .iter()
        .filter(|l| !l.lap.flags.incomplete)
        .map(|l| (l.lap.number, l.lap.invalid_reason.clone()))
        .collect();
    assert_eq!(
        reasons,
        vec![
            (1, None),
            (2, Some("running wide".into())),
            (3, Some("corner cutting".into())),
            (4, Some("corner cutting".into())),
            (5, Some("flashback used".into())),
        ]
    );
}

/// A flashback from just after the line back into the lap before: the game
/// times that lap again. It comes out once, with the new time.
#[test]
fn flashback_back_across_the_line() {
    let mut g = SimGame::new(Format::F2026);
    let mut s = Script::default();
    s.send(&g.session());
    s.send(&g.participants());
    s.drive_lap(&mut g); // lap 1
    while g.dist < 850.0 {
        s.drive(&mut g, 0.05);
    }
    let back = g.clone();
    s.drive_lap(&mut g); // over the line, 1 s into lap 3
    s.send(&g.flashback(back.time));
    g = back;
    g.speed_kmh = 150; // slower this time: 24 s
    g.invalid = true;
    s.send(&g.penalty(10, 31));
    s.drive_lap(&mut g);
    s.drive(&mut g, 2.0);
    s.send(&g.event(b"SEND", &[]));

    let out = record(s);
    let l = laps(&out);
    assert_eq!(
        l.iter().map(|l| summary(l)).collect::<Vec<_>>(),
        vec![
            (1, Some(20_000), true, false, false, false, false),
            (2, Some(24_000), false, false, false, false, true),
            (3, None, false, false, false, true, false),
        ]
    );
    assert_eq!(l[1].lap.invalid_reason.as_deref(), Some("flashback used"));
    // one pass over the lap: 17 s at 50 m/s, 3.6 s at 41.7 m/s, 20 Hz
    let n = l[1].telemetry.len();
    assert!((405..=425).contains(&n), "{n} samples");
}

/// Restarting the lap from the time trial menu: the game puts the car
/// before the line and keeps the lap number. The attempt is incomplete,
/// the run-up isn't recorded, and the lap counts from the line.
#[test]
fn restarting_the_lap() {
    let mut g = SimGame::new(Format::F2026);
    let mut s = Script::default();
    s.send(&g.session());
    s.send(&g.participants());
    s.drive_lap(&mut g); // lap 1
    s.drive(&mut g, 5.0);
    g.invalid = true;
    s.send(&g.penalty(10, 26));
    s.drive(&mut g, 1.0);
    g.restart_lap(300.0);
    s.drive_lap(&mut g); // run-up, then lap 2 again: the penalty is gone
    s.send(&g.event(b"SEND", &[]));

    let out = record(s);
    let l = laps(&out);
    assert_eq!(
        l.iter().map(|l| summary(l)).collect::<Vec<_>>(),
        vec![
            (1, Some(20_000), true, false, false, false, false),
            (2, None, false, false, false, true, false),
            (2, Some(20_000), true, false, false, false, false),
            (3, None, false, false, false, true, false),
        ]
    );
    assert_eq!(
        l[1].lap.invalid_reason.as_deref(),
        Some("incomplete, lap restarted")
    );
    let dist = |i: usize| match &l[i].telemetry.channel("dist").unwrap().data {
        sy_model::ChannelData::F32(v) => v.clone(),
        other => panic!("{other:?}"),
    };
    assert!(dist(2)[0] < 5.0, "starts at the line: {}", dist(2)[0]);
    assert!(dist(2).iter().all(|d| *d < 1000.5), "no run-up");
}

/// Switching the game's UDP format while driving: the session goes on, the
/// lap running at the switch is cut off.
#[test]
fn format_switch_within_a_session() {
    let mut g = SimGame::new(Format::F2025);
    let mut s = Script::default();
    s.send(&g.session());
    s.send(&g.participants());
    s.drive_lap(&mut g); // lap 1, format 2025
    s.drive(&mut g, 5.0);
    g.format = Format::F2026;
    s.drive_lap(&mut g); // lap 2: switched halfway
    s.drive_lap(&mut g); // lap 3, format 2026
    s.send(&g.event(b"SEND", &[]));

    let out = record(s);
    let started = out
        .0
        .iter()
        .filter(|o| matches!(o, RecorderOutput::SessionStarted(_)))
        .count();
    assert_eq!(started, 1);
    let l = laps(&out);
    let complete: Vec<_> = l
        .iter()
        .filter(|l| !l.lap.flags.incomplete)
        .map(|l| (l.lap.number, l.lap.lap_time_ms))
        .collect();
    assert_eq!(complete, vec![(1, Some(20_000)), (3, Some(20_000))]);
    let aero = |l: &RecordedLap| l.telemetry.channel("f1.active_aero_mode").is_some();
    assert!(!aero(l[0]));
    assert!(aero(l.iter().find(|l| l.lap.number == 3).unwrap()));
}

/// Packets from the menus or the garage only: no session.
#[test]
fn menus_only_make_no_session() {
    let mut g = SimGame::new(Format::F2026);
    g.driver_status = 0;
    let mut s = Script::default();
    s.send(&g.session());
    s.send(&g.participants());
    s.drive(&mut g, 10.0);
    let out = record(s);
    assert!(out.0.is_empty(), "{:?}", out.0.first());
}
