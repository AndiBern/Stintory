// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The bridge end to end over real localhost TCP: a simulated LMU in the
//! bridge, the client, and the capture pipeline that the desktop app runs.

use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use sy_bridge::client::Event;
use sy_bridge::protocol::{Connection, Control, Message, PROTOCOL};
use sy_bridge::server::{Bridge, Game};
use sy_bridge::sim::SimGame;
use sy_bridge::{BridgeClient, BridgeError, BridgeView, GameState, SharedView};
use sy_capture::RecorderOutput;
use sy_capture::pipeline::{RunOptions, Sink, run};
use sy_model::GameId;
use sy_store::StoreError;

struct Running {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn start(games: Vec<Box<dyn Game>>) -> Running {
    let mut bridge = Bridge::bind("127.0.0.1:0".parse().unwrap(), games).unwrap();
    let addr = bridge.local_addr().unwrap();
    let stop = bridge.stop_handle();
    let thread = std::thread::spawn(move || bridge.run());
    Running {
        addr,
        stop,
        thread: Some(thread),
    }
}

fn sim(laps: u16) -> Vec<Box<dyn Game>> {
    vec![Box::new(
        SimGame::new(laps, 200.0).with_idle_end(Duration::from_millis(300)),
    )]
}

fn connect(addr: SocketAddr, games: &[GameId]) -> Result<(BridgeClient, SharedView), BridgeError> {
    let view: SharedView = Arc::new(Mutex::new(BridgeView::default()));
    let client = BridgeClient::connect(addr, games, "test", view.clone(), Duration::from_secs(2))?;
    Ok((client, view))
}

#[derive(Default)]
struct Collect(Vec<RecorderOutput>);

impl Sink for Collect {
    fn apply(&mut self, output: RecorderOutput) -> Result<(), StoreError> {
        self.0.push(output);
        Ok(())
    }
}

#[test]
fn a_simulated_session_is_recorded_through_the_bridge() {
    let bridge = start(sim(2));
    let (mut client, view) = connect(bridge.addr, &[GameId::Lmu, GameId::Iracing]).unwrap();
    {
        let v = view.lock().unwrap();
        assert_eq!(
            v.games,
            vec![GameId::Lmu],
            "the bridge reads only what it has"
        );
        assert_eq!(v.bridge_version.as_deref(), Some(env!("CARGO_PKG_VERSION")));
    }

    // wait for the first record, as the desktop supervisor does
    let deadline = Instant::now() + Duration::from_secs(10);
    let first = loop {
        assert!(Instant::now() < deadline, "no record arrived");
        match client.next(Duration::from_millis(500)).unwrap() {
            Some(Event::Record { game, record }) => break (game, record),
            Some(Event::Status { game, state }) => {
                assert_eq!((game, state), (GameId::Lmu, GameState::Driving));
            }
            _ => {}
        }
    };
    assert_eq!(first.0, GameId::Lmu);
    assert_eq!(
        view.lock().unwrap().states.get(&GameId::Lmu),
        Some(&GameState::Driving)
    );

    let mut sink = Collect::default();
    {
        let mut source = client.source(first.0, first.1);
        let ended = source.ended();
        let mut idle_done = |_: Duration| ended.load(Ordering::Relaxed);
        let stats = run(
            &mut source,
            &mut sink,
            RunOptions {
                spool: None,
                should_stop: &|| false,
                idle_done: Some(&mut idle_done),
                progress: None,
            },
        )
        .unwrap();
        assert!(source.take_error().is_none());
        assert!(stats.records > 1000, "{stats:?}");
        assert_eq!(stats.sessions, 1);
    }

    let session = sink
        .0
        .iter()
        .find_map(|o| match o {
            RecorderOutput::SessionStarted(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(session.game, GameId::Lmu);
    assert!(
        session.track_name.contains("Sim Oval"),
        "{}",
        session.track_name
    );
    let timed: Vec<u32> = sink
        .0
        .iter()
        .filter_map(|o| match o {
            RecorderOutput::Lap(l) if !l.lap.flags.out_lap => l.lap.lap_time_ms,
            _ => None,
        })
        .collect();
    assert_eq!(timed.len(), 2, "{timed:?}");
    for t in timed {
        assert!((30_000..50_000).contains(&t), "lap time {t}");
    }

    // the game "closed": the bridge says so and keeps the connection
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < deadline, "no NotRunning status");
        if let Some(Event::Status { state, .. }) = client.next(Duration::from_millis(500)).unwrap()
            && state == GameState::NotRunning
        {
            break;
        }
    }
    drop(client);
    assert_eq!(
        *view.lock().unwrap(),
        BridgeView::default(),
        "reset on disconnect"
    );
}

#[test]
fn nothing_is_read_for_games_the_client_did_not_ask_for() {
    let bridge = start(sim(1));
    let (mut client, view) = connect(bridge.addr, &[GameId::Iracing]).unwrap();
    assert!(view.lock().unwrap().games.is_empty());
    let until = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < until {
        assert_eq!(client.next(Duration::from_millis(200)).unwrap(), None);
    }
}

#[test]
fn another_protocol_version_is_turned_away_with_a_message() {
    let bridge = start(sim(1));
    let mut conn = Connection::new(TcpStream::connect(bridge.addr).unwrap()).unwrap();
    conn.send_control(Control::Hello {
        protocol: PROTOCOL + 1,
        version: "9.9.9".into(),
        games: vec![GameId::Lmu],
    })
    .unwrap();
    match conn.recv(Duration::from_secs(5)).unwrap() {
        Some(Message::Control(Control::Error { message })) => {
            assert!(message.contains("protocol"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn only_one_client_at_a_time() {
    let bridge = start(sim(1));
    // (asks for no game the bridge has: nothing to read, so it needn't be)
    let (_first, _) = connect(bridge.addr, &[GameId::Iracing]).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    match connect(bridge.addr, &[GameId::Lmu]) {
        Err(BridgeError::Remote(message)) => {
            assert!(message.contains("another Stintory"), "{message}");
        }
        other => panic!("{:?}", other.map(|_| ())),
    }
}

#[test]
fn a_vanished_bridge_ends_the_recording_without_an_error() {
    let bridge = start(sim(50));
    let (mut client, _) = connect(bridge.addr, &[GameId::Lmu]).unwrap();
    let first = loop {
        if let Some(Event::Record { game, record }) = client.next(Duration::from_secs(1)).unwrap() {
            break (game, record);
        }
    };
    let mut source = client.source(first.0, first.1);
    let ended = source.ended();
    let mut idle_done = |_: Duration| ended.load(Ordering::Relaxed);
    let killer = {
        let stop = bridge.stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            stop.store(true, Ordering::Relaxed);
        })
    };
    let mut sink = Collect::default();
    let stats = run(
        &mut source,
        &mut sink,
        RunOptions {
            spool: None,
            should_stop: &|| false,
            idle_done: Some(&mut idle_done),
            progress: None,
        },
    )
    .unwrap();
    killer.join().unwrap();
    assert!(stats.records > 0);
    // the bridge said "stopped" on its way out, or the connection closed
    assert!(ended.load(Ordering::Relaxed));
}
