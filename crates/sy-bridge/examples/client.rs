// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A headless Stintory client for trying the bridge: connects to
//! `127.0.0.1:<port>`, prints what the bridge reports and records the
//! sessions through the real pipeline (decoder + recorder), printing the
//! laps instead of storing them.
//!
//! ```sh
//! cargo run -p sy-bridge --example client -- [port] [recordings]
//! ```
//!
//! Ends after `recordings` recordings (default 1).

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sy_bridge::client::Event;
use sy_bridge::{BridgeClient, BridgeError, BridgeView, DEFAULT_PORT};
use sy_capture::RecorderOutput;
use sy_capture::pipeline::{RunOptions, Sink, run};
use sy_model::GameId;
use sy_store::StoreError;

struct Print;

impl Sink for Print {
    fn apply(&mut self, output: RecorderOutput) -> Result<(), StoreError> {
        match output {
            RecorderOutput::SessionStarted(s) => {
                println!("session: {} – {} – {}", s.game, s.track_name, s.car_name);
            }
            RecorderOutput::Lap(rec) => {
                let l = &rec.lap;
                println!(
                    "  lap {:>2}: {:>10} valid={} {} out={} in={} samples={}",
                    l.number,
                    l.lap_time_ms
                        .map(|ms| format!(
                            "{}:{:06.3}",
                            ms / 60_000,
                            f64::from(ms % 60_000) / 1000.0
                        ))
                        .unwrap_or_else(|| "–".into()),
                    l.valid,
                    l.invalid_reason.as_deref().unwrap_or(""),
                    l.flags.out_lap,
                    l.flags.in_lap,
                    rec.telemetry.len(),
                );
            }
            RecorderOutput::SessionEnded(_) => println!("session ended"),
            RecorderOutput::SessionUpdated(_) => {}
        }
        Ok(())
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let port = args
        .next()
        .map_or(DEFAULT_PORT, |p| p.parse().expect("port"));
    let mut left: u32 = args.next().map_or(1, |n| n.parse().expect("recordings"));
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let view = Arc::new(Mutex::new(BridgeView::default()));

    while left > 0 {
        let mut client = match BridgeClient::connect(
            addr,
            &[GameId::Lmu, GameId::Iracing],
            "example",
            view.clone(),
            Duration::from_millis(500),
        ) {
            Ok(c) => c,
            Err(BridgeError::Io(_)) => {
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
            Err(e) => {
                eprintln!("can't use the bridge: {e}");
                std::process::exit(1);
            }
        };
        {
            let v = view.lock().unwrap();
            println!(
                "connected to sy-bridge {} (games {:?})",
                v.bridge_version.as_deref().unwrap_or("?"),
                v.games
            );
        }
        let mut last = std::collections::HashMap::new();
        while left > 0 {
            match client.next(Duration::from_millis(500)) {
                Ok(Some(Event::Status { game, state })) => {
                    if last.insert(game, state) != Some(state) {
                        println!("{game}: {state:?}");
                    }
                }
                Ok(Some(Event::Record { game, record })) => {
                    println!("{game}: recording");
                    let mut source = client.source(game, record);
                    let ended = source.ended();
                    let stats = run(
                        &mut source,
                        &mut Print,
                        RunOptions {
                            spool: None,
                            should_stop: &|| false,
                            idle_done: Some(&mut |_| ended.load(Ordering::Relaxed)),
                            progress: None,
                        },
                    );
                    println!("{game}: recording over: {stats:?}");
                    left -= 1;
                    if let Some(e) = source.take_error() {
                        println!("bridge gone: {e}");
                        break;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    println!("bridge gone: {e}");
                    break;
                }
            }
        }
    }
}
