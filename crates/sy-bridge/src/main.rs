// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sy-bridge.exe`: runs inside a game's Wine/Proton prefix, reads the
//! game's shared memory and streams it to the Stintory app on Linux over
//! `127.0.0.1` (docs/games/proton.md).

use std::net::{Ipv4Addr, SocketAddr};
use std::process::ExitCode;

use sy_bridge::protocol::DEFAULT_PORT;
use sy_bridge::server::{Bridge, Game};

const USAGE: &str = "\
sy-bridge: Stintory's Proton bridge (docs/games/proton.md)

Runs in the game's Wine/Proton prefix, e.g. with the Steam launch option
    PROTON_REMOTE_DEBUG_CMD=/path/to/sy-bridge.exe %command%
and sends the game's telemetry (player's car only) to Stintory on this PC.

Options:
    --port <port>          TCP port on 127.0.0.1 (default 20790; set the
                           same port in Stintory under Settings)
    --simulate-lmu [laps]  no game: drive a simulated Le Mans Ultimate
                           session (default 3 laps; works on Linux too)
    --time-scale <x>       speed of the simulation (default 1)
    --help, --version
";

struct Args {
    port: u16,
    simulate: Option<u16>,
    time_scale: f64,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut out = Args {
        port: DEFAULT_PORT,
        simulate: None,
        time_scale: 1.0,
    };
    let mut args = args.by_ref().peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "--version" | "-V" => {
                println!("sy-bridge {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--port" => {
                let v = args.next().ok_or("--port needs a value")?;
                out.port = v
                    .parse()
                    .ok()
                    .filter(|p| *p >= 1024)
                    .ok_or_else(|| format!("--port {v}: choose a port from 1024 to 65535"))?;
            }
            "--simulate-lmu" => {
                let laps = match args.peek() {
                    Some(v) if !v.starts_with('-') => {
                        let v = args.next().expect("peeked");
                        v.parse()
                            .map_err(|_| format!("--simulate-lmu {v}: not a number"))?
                    }
                    _ => 3,
                };
                out.simulate = Some(laps);
            }
            "--time-scale" => {
                let v = args.next().ok_or("--time-scale needs a value")?;
                out.time_scale = v
                    .parse()
                    .ok()
                    .filter(|x: &f64| x.is_finite() && *x > 0.0)
                    .ok_or_else(|| format!("--time-scale {v}: not a positive number"))?;
            }
            other => return Err(format!("unknown option {other:?} (see --help)")),
        }
    }
    Ok(Some(out))
}

fn games(args: &Args) -> Result<Vec<Box<dyn Game>>, String> {
    if let Some(laps) = args.simulate {
        return Ok(vec![Box::new(sy_bridge::sim::SimGame::new(
            laps,
            args.time_scale,
        ))]);
    }
    #[cfg(windows)]
    {
        Ok(vec![
            Box::new(sy_bridge::windows::Lmu),
            Box::new(sy_bridge::windows::Iracing),
        ])
    }
    #[cfg(not(windows))]
    {
        Err("sy-bridge reads games inside a Wine/Proton prefix; this is the Linux build. Build it for Windows (npm run build:bridge), or try it with --simulate-lmu.".into())
    }
}

fn main() -> ExitCode {
    // Proton writes it to its log (PROTON_LOG=1): no colours there
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = match parse(std::env::args().skip(1)) {
        Ok(Some(a)) => a,
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sy-bridge: {e}");
            return ExitCode::from(2);
        }
    };
    let games = match games(&args) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("sy-bridge: {e}");
            return ExitCode::from(2);
        }
    };
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, args.port));
    let mut bridge = match Bridge::bind(addr, games) {
        Ok(b) => b,
        Err(e) => {
            // e.g. a second game started with the bridge at the same time
            eprintln!("sy-bridge: can't listen on {addr}: {e} (is another sy-bridge running?)");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(
        %addr,
        version = env!("CARGO_PKG_VERSION"),
        "waiting for Stintory"
    );
    bridge.run();
    ExitCode::SUCCESS
}
