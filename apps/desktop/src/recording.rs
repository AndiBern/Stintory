// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Background work: crash recovery at startup and the live recording
//! supervisors (docs/PLAN.md §5): iRacing and Le Mans Ultimate (Windows,
//! shared memory) and F1 25 (every platform, UDP).

use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sy_capture::RecorderOutput;
use sy_capture::pipeline::{RunOptions, SharedStore, Sink, finish_spool, run};
use sy_capture::spool::SpoolWriter;
use sy_model::GameId;
use sy_model::api::{F1CaptureSettings, RecorderState, RecorderStatus};
use sy_model::capture::RawSource;
use sy_source_f1::{UdpConfig, UdpSource};
use sy_store::{F1_CAPTURE, SpoolState, StoreError};
use tauri::{AppHandle, Emitter};

use crate::commands::EVENT_SESSIONS_CHANGED;
use crate::sync::SyncManager;

/// Event: live recorder status changed ([`RecorderStatus`]).
pub const EVENT_RECORDER_STATUS: &str = "recorder-status";

/// Replays recordings left over from a crash, in the background.
pub fn spawn_recovery(app: AppHandle, store: SharedStore) {
    std::thread::Builder::new()
        .name("recovery".into())
        .spawn(move || match sy_capture::pipeline::recover_spools(&store) {
            Ok(0) => {}
            Ok(laps) => {
                tracing::info!(laps, "recovered laps from unprocessed recordings");
                let _ = app.emit(EVENT_SESSIONS_CHANGED, ());
            }
            Err(e) => tracing::error!(error = %e, "crash recovery failed"),
        })
        .expect("spawn recovery thread");
}

/// Updates the shared status and notifies the frontend.
#[derive(Clone)]
struct StatusReporter {
    app: AppHandle,
    status: Arc<Mutex<RecorderStatus>>,
}

impl StatusReporter {
    fn update(&self, f: impl FnOnce(&mut RecorderStatus)) {
        let snapshot = {
            let mut s = self.status.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut s);
            s.clone()
        };
        let _ = self.app.emit(EVENT_RECORDER_STATUS, snapshot);
    }

    /// Back to waiting, unless another game is being recorded.
    fn idle(&self, game: GameId) {
        self.update(|s| {
            if s.state != RecorderState::Recording || s.game == Some(game) {
                let hint = s.hint.take();
                *s = RecorderStatus::idle(RecorderState::Waiting);
                s.hint = hint;
            }
        });
    }

    /// Sets the hint shown while waiting (only sends an event on change).
    #[cfg_attr(not(windows), allow(dead_code))]
    fn hint(&self, hint: Option<&str>) {
        let changed = {
            let s = self.status.lock().unwrap_or_else(|e| e.into_inner());
            s.hint.as_deref() != hint
        };
        if changed {
            self.update(|s| s.hint = hint.map(str::to_owned));
        }
    }
}

/// A sink that stores, and reports progress to the UI.
struct ReportingSink {
    store: SharedStore,
    reporter: StatusReporter,
    sync: Arc<SyncManager>,
}

impl Sink for ReportingSink {
    fn apply(&mut self, output: RecorderOutput) -> Result<(), StoreError> {
        let notify = match &output {
            RecorderOutput::SessionStarted(s) => {
                let (track, car, game) = (s.track_name.clone(), s.car_name.clone(), s.game);
                self.reporter.update(|st| {
                    st.state = RecorderState::Recording;
                    st.game = Some(game);
                    st.track_name = Some(track);
                    st.car_name = Some(car);
                    st.laps = 0;
                    st.last_lap_ms = None;
                });
                true
            }
            RecorderOutput::Lap(lap) => {
                let time = lap.lap.lap_time_ms;
                self.reporter.update(|st| {
                    st.laps += 1;
                    st.last_lap_ms = time.or(st.last_lap_ms);
                });
                true
            }
            RecorderOutput::SessionUpdated(_) => false,
            RecorderOutput::SessionEnded(_) => true,
        };
        self.store.apply(output)?;
        if notify {
            let _ = self.reporter.app.emit(EVENT_SESSIONS_CHANGED, ());
            self.sync.nudge();
        }
        Ok(())
    }
}

/// What the recording threads share.
#[derive(Clone)]
struct Context {
    store: SharedStore,
    reporter: StatusReporter,
    shutdown: Arc<AtomicBool>,
    sync: Arc<SyncManager>,
}

impl Context {
    fn stopping(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }

    /// Records from a live source (spooled) until `idle_done` says the
    /// silence means the end, or the app shuts down.
    fn record(
        &self,
        game: GameId,
        source: &mut dyn RawSource,
        idle_done: &mut dyn FnMut(Duration) -> bool,
    ) {
        let spool = match self.store.with(|s| s.register_spool(game)) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "cannot register spool");
                std::thread::sleep(Duration::from_secs(10));
                return;
            }
        };
        let mut writer = match SpoolWriter::create(&spool.path, game) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!(error = %e, "cannot create spool file");
                std::thread::sleep(Duration::from_secs(10));
                return;
            }
        };
        let mut sink = ReportingSink {
            store: self.store.clone(),
            reporter: self.reporter.clone(),
            sync: self.sync.clone(),
        };
        let stop = || self.stopping();
        let result = run(
            source,
            &mut sink,
            RunOptions {
                spool: Some(&mut writer),
                should_stop: &stop,
                idle_done: Some(idle_done),
                progress: None,
            },
        );
        let _ = writer.finish();
        match result {
            Ok(stats) => {
                tracing::info!(
                    %game,
                    records = stats.records,
                    laps = stats.laps,
                    "recording finished"
                );
                if let Err(e) = finish_spool(&self.store, &spool, &stats) {
                    tracing::error!(error = %e, "cannot finish spool");
                }
            }
            // On shutdown the spool stays in state `Recording`; it is
            // replayed at the next start (idempotent).
            Err(sy_capture::pipeline::PipelineError::Cancelled) => {}
            Err(e) => {
                tracing::error!(error = %e, "recording failed");
                let msg = e.to_string();
                let _ = self
                    .store
                    .with(|s| s.set_spool_state(&spool.id, SpoolState::Failed, Some(&msg)));
            }
        }
        self.reporter.idle(game);
    }
}

/// State of F1 recording shared with the settings commands.
#[derive(Debug, Default)]
pub struct F1Capture {
    /// Bumped when the settings change
    generation: AtomicU64,
    /// (listening on, error)
    status: Mutex<(Option<SocketAddr>, Option<String>)>,
}

impl F1Capture {
    /// The settings changed: applied as soon as no session is recorded.
    pub fn changed(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Where the app listens, and why it can't.
    pub fn status(&self) -> (Option<String>, Option<String>) {
        let s = self.status.lock().unwrap_or_else(|e| e.into_inner());
        (s.0.map(|a| a.to_string()), s.1.clone())
    }

    fn set_status(&self, listening: Option<SocketAddr>, error: Option<String>) {
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = (listening, error);
    }
}

/// Checks F1 settings and turns them into the UDP source's config.
pub fn f1_config(s: &F1CaptureSettings) -> Result<UdpConfig, String> {
    if s.port < 1024 {
        return Err("choose a port from 1024 to 65535".into());
    }
    let ip = if s.allow_remote {
        std::net::Ipv4Addr::UNSPECIFIED
    } else {
        std::net::Ipv4Addr::LOCALHOST
    };
    let listen = SocketAddr::from((ip, s.port));
    let mut forward = Vec::new();
    for target in s.forward.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
        let addr = target
            .to_socket_addrs()
            .ok()
            .and_then(|mut a| a.next())
            .ok_or_else(|| format!("{target:?} is not an address like 127.0.0.1:20778"))?;
        let to_self = addr.port() == s.port
            && (addr.ip().is_loopback() || addr.ip().is_unspecified() || s.allow_remote);
        if to_self {
            return Err(format!(
                "{target} is Stintory's own port; forward to another app's port"
            ));
        }
        if addr.port() == 0 {
            return Err(format!("{target}: the port is missing"));
        }
        if !forward.contains(&addr) {
            forward.push(addr);
        }
    }
    Ok(UdpConfig { listen, forward })
}

/// State of LMU recording shared with the settings commands.
#[derive(Debug, Default)]
pub struct LmuCapture {
    /// Bumped when the settings change
    generation: AtomicU64,
    /// What the app sees of the game, and what to do in the game
    status: Mutex<(Option<String>, Option<String>)>,
}

impl LmuCapture {
    /// The settings changed.
    pub fn changed(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// (status, hint) for the settings page.
    pub fn status(&self) -> (Option<String>, Option<String>) {
        if !cfg!(windows) {
            return (
                Some("Le Mans Ultimate can only be recorded on Windows.".into()),
                None,
            );
        }
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    fn set_status(&self, status: &str, hint: Option<&str>) {
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) =
            (Some(status.to_owned()), hint.map(str::to_owned));
    }
}

/// Shown when LMU runs without its shared memory.
#[cfg_attr(not(windows), allow(dead_code))]
pub const LMU_PLUGINS_HINT: &str = "Le Mans Ultimate is running but shares no telemetry. In the game, turn on Settings → Gameplay → Enable Plugins, then restart the game.";

/// No F1 packets for this long after a session ended: the recording ends
/// (game closed, or in the menus).
const F1_IDLE_END: Duration = Duration::from_secs(30);
/// The game sends nothing while paused. Within a session the recording
/// waits this long before it assumes the game was closed.
const F1_PAUSE_MAX: Duration = Duration::from_secs(3600);

/// Listens for F1 packets per the settings; records while they arrive.
fn supervise_f1(ctx: Context, f1: Arc<F1Capture>) {
    while !ctx.stopping() {
        let generation = f1.generation.load(Ordering::Relaxed);
        let settings: F1CaptureSettings = ctx
            .store
            .with(|s| s.setting(F1_CAPTURE))
            .ok()
            .flatten()
            .unwrap_or_default();
        let config = match f1_config(&settings) {
            Ok(c) if settings.enabled => c,
            Ok(_) => {
                f1.set_status(None, None);
                wait_for_change(&ctx, &f1.generation, generation, Duration::from_secs(3600));
                continue;
            }
            Err(e) => {
                f1.set_status(None, Some(e));
                wait_for_change(&ctx, &f1.generation, generation, Duration::from_secs(3600));
                continue;
            }
        };
        let mut source = match UdpSource::bind(&config) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(addr = %config.listen, error = %e, "cannot listen for F1 packets");
                f1.set_status(None, Some(format!("can't use {}: {e}", config.listen)));
                wait_for_change(&ctx, &f1.generation, generation, Duration::from_secs(10));
                continue;
            }
        };
        tracing::info!(addr = %config.listen, "listening for F1 25 packets");
        f1.set_status(Some(config.listen), None);
        while !ctx.stopping() && f1.generation.load(Ordering::Relaxed) == generation {
            let result = source.wait_for_data(Duration::from_millis(500));
            f1.set_status(
                Some(config.listen),
                source.unsupported_format().map(|f| {
                    format!(
                        "The game sends UDP format {f}, which Stintory can't read. In the game's UDP Telemetry Settings, set UDP Format to 2026 or 2025."
                    )
                }),
            );
            match result {
                Ok(true) => {
                    tracing::info!("F1 25 is sending telemetry, recording");
                    let open = source.session_open();
                    let mut idle_done = |idle: Duration| {
                        idle >= if open.load(Ordering::Relaxed) {
                            F1_PAUSE_MAX
                        } else {
                            F1_IDLE_END
                        }
                    };
                    ctx.record(GameId::F1_25, &mut source, &mut idle_done);
                }
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "receiving F1 packets failed");
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }
}

/// Sleeps until the settings change, the app stops, or `max` passed.
fn wait_for_change(ctx: &Context, settings: &AtomicU64, generation: u64, max: Duration) {
    let start = std::time::Instant::now();
    while !ctx.stopping() && settings.load(Ordering::Relaxed) == generation && start.elapsed() < max
    {
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Starts the live recording supervisors: F1 25 on every platform, iRacing
/// and Le Mans Ultimate on Windows.
#[allow(clippy::too_many_arguments)]
pub fn spawn_supervisor(
    app: AppHandle,
    store: SharedStore,
    status: Arc<Mutex<RecorderStatus>>,
    shutdown: Arc<AtomicBool>,
    sync: Arc<SyncManager>,
    f1: Arc<F1Capture>,
    lmu: Arc<LmuCapture>,
) {
    let ctx = Context {
        store,
        reporter: StatusReporter { app, status },
        shutdown,
        sync,
    };
    ctx.reporter
        .update(|s| *s = RecorderStatus::idle(RecorderState::Waiting));
    let f1_ctx = ctx.clone();
    std::thread::Builder::new()
        .name("recorder-f1".into())
        .spawn(move || supervise_f1(f1_ctx, f1))
        .expect("spawn F1 recorder thread");
    #[cfg(windows)]
    {
        let lmu_ctx = ctx.clone();
        std::thread::Builder::new()
            .name("recorder-lmu".into())
            .spawn(move || supervise_lmu(lmu_ctx, lmu))
            .expect("spawn LMU recorder thread");
        std::thread::Builder::new()
            .name("recorder-iracing".into())
            .spawn(move || supervise_iracing(ctx))
            .expect("spawn iRacing recorder thread");
    }
    #[cfg(not(windows))]
    drop(lmu);
}

/// Records Le Mans Ultimate while the player drives; tells the user when
/// the game runs without its shared memory.
#[cfg(windows)]
fn supervise_lmu(ctx: Context, lmu: Arc<LmuCapture>) {
    use sy_source_lmu::{LiveSource, Probe};

    /// No data for this long while not driving: the session is over (game
    /// closed, back in the menus).
    const IDLE_END: Duration = Duration::from_secs(10);
    /// The game sends nothing while paused; while driving, wait this long.
    const PAUSE_MAX: Duration = Duration::from_secs(3600);

    while !ctx.stopping() {
        let generation = lmu.generation.load(Ordering::Relaxed);
        let settings: sy_model::api::LmuCaptureSettings = ctx
            .store
            .with(|s| s.setting(sy_store::LMU_CAPTURE))
            .ok()
            .flatten()
            .unwrap_or_default();
        if !settings.enabled {
            lmu.set_status("Off.", None);
            ctx.reporter.hint(None);
            wait_for_change(&ctx, &lmu.generation, generation, Duration::from_secs(3600));
            continue;
        }
        match LiveSource::probe() {
            Probe::NotRunning => {
                lmu.set_status("Waiting for Le Mans Ultimate.", None);
                ctx.reporter.hint(None);
                wait_for_change(&ctx, &lmu.generation, generation, Duration::from_secs(2));
            }
            Probe::NoSharedMemory => {
                lmu.set_status("Le Mans Ultimate is running.", Some(LMU_PLUGINS_HINT));
                ctx.reporter.hint(Some(LMU_PLUGINS_HINT));
                wait_for_change(&ctx, &lmu.generation, generation, Duration::from_secs(2));
            }
            Probe::UnknownLayout => {
                lmu.set_status(
                    "This version of Le Mans Ultimate shares its data in a layout Stintory doesn't know yet. Please update Stintory.",
                    None,
                );
                ctx.reporter.hint(None);
                wait_for_change(&ctx, &lmu.generation, generation, Duration::from_secs(10));
            }
            Probe::Idle => {
                lmu.set_status(
                    "Connected to Le Mans Ultimate; recording starts on track.",
                    None,
                );
                ctx.reporter.hint(None);
                wait_for_change(&ctx, &lmu.generation, generation, Duration::from_secs(1));
            }
            Probe::Driving => {
                lmu.set_status("Recording Le Mans Ultimate.", None);
                ctx.reporter.hint(None);
                tracing::info!("Le Mans Ultimate: player on track, recording");
                let mut source = LiveSource::new();
                let driving = source.driving();
                let mut idle_done = |idle: Duration| {
                    idle >= if driving.load(Ordering::Relaxed) {
                        PAUSE_MAX
                    } else {
                        IDLE_END
                    }
                };
                ctx.record(GameId::Lmu, &mut source, &mut idle_done);
            }
        }
    }
}

#[cfg(windows)]
fn supervise_iracing(ctx: Context) {
    use sy_source_iracing::LiveSource;

    /// No data for this long: the session is over (sim closed or left).
    const IDLE_END: Duration = Duration::from_secs(10);

    while !ctx.stopping() {
        if !LiveSource::probe() {
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }
        tracing::info!("iRacing is sending telemetry, recording");
        let mut source = LiveSource::new();
        ctx.record(GameId::Iracing, &mut source, &mut |idle| idle >= IDLE_END);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(port: u16, allow_remote: bool, forward: &[&str]) -> F1CaptureSettings {
        F1CaptureSettings {
            enabled: true,
            port,
            allow_remote,
            forward: forward.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn f1_settings_are_checked() {
        let c = f1_config(&settings(20777, false, &[])).unwrap();
        assert_eq!(c.listen, "127.0.0.1:20777".parse().unwrap());
        let c = f1_config(&settings(
            20777,
            true,
            &["127.0.0.1:20778", " ", "127.0.0.1:20778"],
        ))
        .unwrap();
        assert_eq!(c.listen, "0.0.0.0:20777".parse().unwrap());
        assert_eq!(c.forward, vec!["127.0.0.1:20778".parse().unwrap()]);
        assert!(f1_config(&settings(80, false, &[])).is_err());
        assert!(
            f1_config(&settings(20777, false, &["127.0.0.1:20777"])).is_err(),
            "loop"
        );
        assert!(
            f1_config(&settings(20777, true, &["192.168.1.5:20777"])).is_err(),
            "loop"
        );
        assert!(f1_config(&settings(20777, false, &["192.168.1.5:20777"])).is_ok());
        assert!(f1_config(&settings(20777, false, &["no address"])).is_err());
    }
}
