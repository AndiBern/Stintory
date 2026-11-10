// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Rundenzeit desktop client (Tauri backend).
//!
//! The web frontend (`web/`) calls the commands in [`commands`] through
//! Tauri's `invoke`. They implement the same API shape as rz-server's REST
//! API, see `web/src/lib/api/`.

mod commands;
mod state;

use std::path::PathBuf;

use tauri::Manager;
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

/// Environment variable to override the data directory (useful for development).
const DATA_DIR_ENV: &str = "RUNDENZEIT_DATA_DIR";

/// Starts the desktop app.
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        .setup(|app| {
            let data_dir = match std::env::var_os(DATA_DIR_ENV) {
                Some(dir) => PathBuf::from(dir),
                None => app.path().app_data_dir()?,
            };
            tracing::info!(data_dir = %data_dir.display(), "opening local store");
            let state = AppState::open(&data_dir)?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_info,
            commands::list_sessions
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Rundenzeit desktop app");
}
