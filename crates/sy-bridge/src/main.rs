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
Ends with the game (when Proton's steam.exe, which runs the game, is gone).

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
    #[cfg(windows)]
    if args.simulate.is_none() && detach_from_proton() {
        return ExitCode::SUCCESS;
    }
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
    #[cfg(windows)]
    if args.simulate.is_none() {
        exit_with_game(bridge.stop_handle());
    }
    bridge.run();
    ExitCode::SUCCESS
}

/// Started by Proton (`PROTON_REMOTE_DEBUG_CMD`): Wine runs a program given
/// by a Unix path through `start.exe /exec`, which waits for it and counts
/// as a normal process, so Proton's `steam.exe` would wait for it for ever
/// (`sy_bridge::lifetime`). So the bridge continues as a detached copy of
/// itself and this process ends, and `start.exe` with it. Returns true when
/// the copy runs and this process should end.
#[cfg(windows)]
fn detach_from_proton() -> bool {
    use std::os::windows::process::CommandExt;

    /// Set in the copy's environment
    const DETACHED: &str = "STINTORY_BRIDGE_DETACHED";
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    if std::env::var_os("PROTON_REMOTE_DEBUG_CMD").is_none() || std::env::var_os(DETACHED).is_some()
    {
        return false;
    }
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "can't find myself; staying attached to Proton");
            return false;
        }
    };
    match std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env(DETACHED, "1")
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
    {
        Ok(child) => {
            tracing::info!(pid = child.id(), "started by Proton; continuing detached");
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, "can't detach; staying attached to Proton");
            false
        }
    }
}

/// Ends the bridge when the game ended (`sy_bridge::lifetime`).
#[cfg(windows)]
fn exit_with_game(stop: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use sy_bridge::lifetime::{ExitWithGame, GAME_HELPER};

    std::thread::Builder::new()
        .name("exit-with-game".into())
        .spawn(move || {
            let mut watch = ExitWithGame::new();
            let mut system = false;
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let helper = sy_bridge::windows::process_running(GAME_HELPER);
                // once the game runs, stop counting as one of the
                // processes Proton's steam.exe waits for
                if helper && !system {
                    system = true;
                    match sy_bridge::windows::make_process_system() {
                        Ok(()) => tracing::info!("the game started; the bridge ends with it"),
                        Err(status) => tracing::warn!(
                            status = format_args!("{status:#x}"),
                            "can't become a Wine system process; Proton may wait for the bridge"
                        ),
                    }
                }
                if watch.check(helper) {
                    tracing::info!("the game ended; ending the bridge");
                    stop.store(true, Ordering::Relaxed);
                    // the loop notices within a second; don't hang on it
                    std::thread::sleep(Duration::from_secs(5));
                    std::process::exit(0);
                }
            }
        })
        .expect("spawn the exit-with-game thread");
}
