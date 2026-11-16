// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stintory desktop client (Tauri backend).
//!
//! The web frontend (`web/`) calls the commands in [`commands`] through
//! Tauri's `invoke`. They implement the same API shape as sy-server's REST
//! API, see `web/src/lib/api/`.

mod commands;
mod mcp;
mod recording;
mod state;
mod sync;
mod tray;

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use tauri::{Manager, RunEvent};
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

/// Environment variable to override the data directory (useful for development).
pub(crate) const DATA_DIR_ENV: &str = "STINTORY_DATA_DIR";

/// The app id when running as a Flatpak (`$FLATPAK_ID`).
pub(crate) fn flatpak_id() -> Option<String> {
    std::env::var("FLATPAK_ID").ok().filter(|id| !id.is_empty())
}

/// With NVIDIA's driver, WebKitGTK's DMA-BUF renderer breaks the Wayland
/// connection ("Error 71 (Protocol error) dispatching to Wayland display")
/// and the window never opens. So it's turned off there, unless
/// `WEBKIT_DISABLE_DMABUF_RENDERER` is already set; other GPUs keep it.
/// Returns whether it was turned off. Must run before any other thread
/// exists.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)] // set_var, see SAFETY comment
fn disable_dmabuf_on_nvidia() -> bool {
    const VAR: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";
    // the kernel driver's entry, visible inside the Flatpak too
    let nvidia = std::path::Path::new("/proc/driver/nvidia/version").exists();
    if !nvidia || std::env::var_os(VAR).is_some() {
        return false;
    }
    // SAFETY: called first thing in `run`, while the process has one thread
    unsafe { std::env::set_var(VAR, "1") };
    true
}

/// Runs the MCP server for AI assistants on stdin/stdout (`stintory mcp`,
/// see [`mcp`]); returns the exit code.
pub fn run_mcp() -> i32 {
    mcp::run()
}

/// Starts the desktop app.
pub fn run() {
    #[cfg(target_os = "linux")]
    let dmabuf_off = disable_dmabuf_on_nvidia();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    #[cfg(target_os = "linux")]
    if dmabuf_off {
        tracing::info!("NVIDIA driver: WebKit's DMA-BUF renderer turned off");
    }

    let mut builder = tauri::Builder::default();
    // A second start shows the running app's window instead (it may be
    // hidden in the tray). Two apps on the same data directory would both
    // record. Skipped with an own data directory, so a development build
    // can run next to the installed app.
    if std::env::var_os(DATA_DIR_ENV).is_none() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main_window(app);
        }));
    }
    let app = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = match std::env::var_os(DATA_DIR_ENV) {
                Some(dir) => PathBuf::from(dir),
                None => app.path().app_data_dir()?,
            };
            tracing::info!(data_dir = %data_dir.display(), "opening local store");
            let state = AppState::open(&data_dir)?;
            if cfg!(not(windows)) {
                let resources = app.path().resource_dir().ok();
                let exe = recording::install_bridge(resources.as_deref(), &data_dir);
                if let Err(e) = &exe {
                    tracing::warn!(error = %e, "Proton bridge not available");
                }
                state.bridge.set_exe(exe);
            }
            let sync = sync::SyncManager::start(app.handle().clone(), state.store.clone());
            recording::spawn_recovery(app.handle().clone(), state.store.clone());
            recording::spawn_supervisor(
                app.handle().clone(),
                state.store.clone(),
                state.recorder.clone(),
                state.shutdown.clone(),
                sync.clone(),
                state.f1.clone(),
                state.lmu.clone(),
                state.bridge.clone(),
            );
            let close_to_tray = state
                .store
                .with(|s| s.setting(sy_store::CLOSE_TO_TRAY))
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "couldn't read the tray setting");
                    None
                })
                .unwrap_or(false);
            if close_to_tray && let Err(e) = tray::enable(app.handle()) {
                tracing::warn!(error = %e, "close to tray not available");
            }
            app.manage(state);
            app.manage(sync);
            Ok(())
        })
        .on_window_event(tray::on_window_event)
        .invoke_handler(tauri::generate_handler![
            commands::get_info,
            commands::list_sessions,
            commands::get_session,
            commands::delete_session,
            commands::compare_laps,
            commands::reference_laps,
            commands::import_files,
            commands::export_bundle,
            commands::export_csv,
            commands::sync_status,
            commands::sync_connect,
            commands::sync_cancel_connect,
            commands::sync_connect_token,
            commands::sync_disconnect,
            commands::sync_now,
            commands::sync_set_auto,
            commands::server_request,
            commands::open_shared_session,
            commands::get_settings,
            commands::set_keep_raw_recordings,
            commands::set_close_to_tray,
            commands::set_f1_capture,
            commands::set_lmu_capture,
            commands::set_proton_bridge,
            commands::delete_raw_recordings,
            commands::open_data_dir,
            commands::recorder_status,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the Stintory desktop app");

    app.run(|handle, event| {
        if let RunEvent::Exit = event
            && let Some(state) = handle.try_state::<AppState>()
        {
            state.shutdown.store(true, Ordering::Relaxed);
        }
    });
}
