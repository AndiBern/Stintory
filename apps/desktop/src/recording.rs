// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Background work: crash recovery at startup and the live recording
//! supervisor (docs/PLAN.md §5).

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use rz_capture::RecorderOutput;
use rz_capture::pipeline::{SharedStore, Sink};
use rz_model::api::{RecorderState, RecorderStatus};
use rz_store::StoreError;
use tauri::{AppHandle, Emitter};

use crate::commands::EVENT_SESSIONS_CHANGED;
use crate::sync::SyncManager;

/// Event: live recorder status changed ([`RecorderStatus`]).
pub const EVENT_RECORDER_STATUS: &str = "recorder-status";

/// Replays recordings left over from a crash, in the background.
pub fn spawn_recovery(app: AppHandle, store: SharedStore) {
    std::thread::Builder::new()
        .name("recovery".into())
        .spawn(move || match rz_capture::pipeline::recover_spools(&store) {
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
}

/// A sink that stores, and reports progress to the UI.
// Only live recording (Windows for now) uses it.
#[cfg_attr(not(windows), allow(dead_code))]
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

/// Starts the live recording supervisor. On platforms without live sources
/// (Linux, for now) this only sets the status.
pub fn spawn_supervisor(
    app: AppHandle,
    store: SharedStore,
    status: Arc<Mutex<RecorderStatus>>,
    shutdown: Arc<AtomicBool>,
    sync: Arc<SyncManager>,
) {
    let reporter = StatusReporter { app, status };
    if !cfg!(windows) {
        reporter.update(|s| *s = RecorderStatus::idle(RecorderState::Unsupported));
        return;
    }
    std::thread::Builder::new()
        .name("recorder".into())
        .spawn(move || supervise(store, reporter, shutdown, sync))
        .expect("spawn recorder thread");
}

#[cfg(not(windows))]
fn supervise(
    _store: SharedStore,
    _reporter: StatusReporter,
    _shutdown: Arc<AtomicBool>,
    _sync: Arc<SyncManager>,
) {
}

#[cfg(windows)]
fn supervise(
    store: SharedStore,
    reporter: StatusReporter,
    shutdown: Arc<AtomicBool>,
    sync: Arc<SyncManager>,
) {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use rz_capture::pipeline::{RunOptions, finish_spool, run};
    use rz_capture::spool::SpoolWriter;
    use rz_model::GameId;
    use rz_source_iracing::LiveSource;
    use rz_store::SpoolState;

    /// No data for this long: the session is over (sim closed or left).
    const IDLE_END: Duration = Duration::from_secs(10);

    reporter.update(|s| *s = RecorderStatus::idle(RecorderState::Waiting));
    while !shutdown.load(Ordering::Relaxed) {
        if !LiveSource::probe() {
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }
        tracing::info!("iRacing is sending telemetry, recording");
        let spool = match store.with(|s| s.register_spool(GameId::Iracing)) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "cannot register spool");
                std::thread::sleep(Duration::from_secs(10));
                continue;
            }
        };
        let mut writer = match SpoolWriter::create(&spool.path, GameId::Iracing) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!(error = %e, "cannot create spool file");
                std::thread::sleep(Duration::from_secs(10));
                continue;
            }
        };
        let mut source = LiveSource::new();
        let mut sink = ReportingSink {
            store: store.clone(),
            reporter: reporter.clone(),
            sync: sync.clone(),
        };
        let stop = || shutdown.load(Ordering::Relaxed);
        let mut idle_done = |idle: Duration| idle >= IDLE_END;
        let result = run(
            &mut source,
            &mut sink,
            RunOptions {
                spool: Some(&mut writer),
                should_stop: &stop,
                idle_done: Some(&mut idle_done),
                progress: None,
            },
        );
        let _ = writer.finish();
        match result {
            Ok(stats) => {
                tracing::info!(
                    records = stats.records,
                    laps = stats.laps,
                    "recording finished"
                );
                if let Err(e) = finish_spool(&store, &spool, &stats) {
                    tracing::error!(error = %e, "cannot finish spool");
                }
            }
            // On shutdown the spool stays in state `Recording`; it is
            // replayed at the next start (idempotent).
            Err(rz_capture::pipeline::PipelineError::Cancelled) => {}
            Err(e) => {
                tracing::error!(error = %e, "recording failed");
                let msg = e.to_string();
                let _ =
                    store.with(|s| s.set_spool_state(&spool.id, SpoolState::Failed, Some(&msg)));
            }
        }
        reporter.update(|s| *s = RecorderStatus::idle(RecorderState::Waiting));
    }
}
