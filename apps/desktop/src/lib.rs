// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stintory desktop client (Tauri backend).
//!
//! The web frontend (`web/`) calls the commands in [`commands`] through
//! Tauri's `invoke`. They implement the same API shape as sy-server's REST
//! API, see `web/src/lib/api/`.

#[cfg(target_os = "linux")]
mod appimage;
mod commands;
mod mcp;
mod recording;
mod state;
mod sync;

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use tauri::{Manager, RunEvent};
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

/// Environment variable to override the data directory (useful for development).
pub(crate) const DATA_DIR_ENV: &str = "STINTORY_DATA_DIR";

/// Runs the MCP server for AI assistants on stdin/stdout (`stintory mcp`,
/// see [`mcp`]); returns the exit code.
pub fn run_mcp() -> i32 {
    mcp::run()
}

/// Starts the desktop app.
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // before the window opens: the compositor looks the icon up when it maps
    #[cfg(target_os = "linux")]
    appimage::integrate();

    let app = tauri::Builder::default()
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
            app.manage(state);
            app.manage(sync);
            Ok(())
        })
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
            commands::set_f1_capture,
            commands::set_lmu_capture,
            commands::set_proton_bridge,
            commands::delete_raw_recordings,
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
