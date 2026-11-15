// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! MCP server over stdio (`stintory mcp`): an AI assistant on this computer
//! (Claude Code, Claude Desktop, …) starts the app in this mode and reads the
//! local sessions, laps and the corner analysis. No window, no recording,
//! read-only; the app may run at the same time (the store is SQLite in WAL
//! mode, readers don't block it).
//!
//! The tools and the protocol are sy-mcp's, as on the server. Messages are
//! newline-delimited JSON-RPC on stdin/stdout (MCP stdio transport); logs go
//! to stderr.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::Value;
use sy_analysis::{DEFAULT_STEP_M, MIN_STEP_M};
use sy_model::api::{CompareData, ReferenceLap, ReferenceScope, SessionDetail, SessionSummary};
use sy_model::remote::SharedTrack;
use sy_model::{LapId, SessionId};
use sy_store::{DB_FILE, Store, StoreError};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

use crate::DATA_DIR_ENV;
use crate::commands::{compare_loaded, comparison_laps, session_detail};

/// Must equal `identifier` in tauri.conf.json: Tauri keeps the app's data
/// in `<platform data directory>/<identifier>`.
const IDENTIFIER: &str = "de.stintory.Stintory";

/// Most laps compared at once (as in the app).
const MAX_LAPS: usize = 8;

/// The app's data directory, as the window app uses it.
pub(crate) fn data_dir() -> Option<PathBuf> {
    std::env::var_os(DATA_DIR_ENV)
        .map(PathBuf::from)
        .or_else(|| dirs::data_dir().map(|d| d.join(IDENTIFIER)))
}

/// The local store as the tools' data.
struct Local {
    store: Mutex<Store>,
}

impl Local {
    fn with<T>(&self, f: impl FnOnce(&Store) -> Result<T, StoreError>) -> Result<T, String> {
        let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        f(&store).map_err(|e| e.to_string())
    }
}

impl sy_mcp::Data for Local {
    async fn sessions(&self) -> Result<Vec<SessionSummary>, String> {
        self.with(Store::list_sessions)
    }

    async fn session(&self, id: Uuid) -> Result<SessionDetail, String> {
        self.with(|s| session_detail(s, SessionId::from_uuid(id)))
    }

    async fn shared_tracks(&self) -> Result<Vec<SharedTrack>, String> {
        // shared laps live on the server; kept copies count as sessions
        Ok(Vec::new())
    }

    async fn reference_laps(
        &self,
        track_key: &str,
        car_key: &str,
        scope: ReferenceScope,
    ) -> Result<Vec<ReferenceLap>, String> {
        self.with(|s| s.reference_laps(track_key, car_key, scope))
    }

    async fn compare(&self, lap_ids: Vec<Uuid>, step_m: f32) -> Result<CompareData, String> {
        if lap_ids.is_empty() || lap_ids.len() > MAX_LAPS {
            return Err(format!("compare 1 to {MAX_LAPS} laps"));
        }
        let ids = lap_ids.into_iter().map(LapId::from_uuid).collect();
        let laps = self.with(|s| comparison_laps(s, ids))?;
        let step = if step_m.is_finite() {
            step_m.max(MIN_STEP_M)
        } else {
            DEFAULT_STEP_M
        };
        compare_loaded(&laps, step).map_err(|e| e.to_string())
    }
}

/// Runs the MCP server on stdin/stdout until stdin closes. Returns the exit
/// code.
pub fn run() -> i32 {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();
    let Some(dir) = data_dir() else {
        eprintln!("stintory mcp: no data directory (set {DATA_DIR_ENV})");
        return 1;
    };
    // don't create an empty store where the app has none
    if !dir.join(DB_FILE).is_file() {
        eprintln!(
            "stintory mcp: no Stintory data in {}: open the app and record or import laps first",
            dir.display()
        );
        return 1;
    }
    let store = match Store::open(&dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "stintory mcp: can't open the data in {}: {e}",
                dir.display()
            );
            return 1;
        }
    };
    let local = Local {
        store: Mutex::new(store),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("stintory mcp: {e}");
            return 1;
        }
    };
    tracing::info!(data_dir = %dir.display(), "MCP server on stdio");
    let mut out = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(message) => runtime.block_on(sy_mcp::handle_message(&local, &message)),
            Err(_) => Some(sy_mcp::error_message(
                None,
                sy_mcp::PARSE_ERROR,
                "invalid JSON",
                None,
            )),
        };
        if let Some(reply) = reply
            && (writeln!(out, "{reply}").is_err() || out.flush().is_err())
        {
            break;
        }
    }
    0
}

/// How an assistant starts this mode: the program and its arguments. The
/// Flatpak's program exists only inside the sandbox, so there it's
/// `flatpak run <app id> mcp`.
pub(crate) fn launch_command() -> Option<(String, Vec<String>)> {
    if let Some(id) = crate::flatpak_id() {
        return Some((
            "flatpak".to_owned(),
            vec!["run".to_owned(), id, "mcp".to_owned()],
        ));
    }
    let exe = std::env::current_exe().ok()?;
    Some((exe.display().to_string(), vec!["mcp".to_owned()]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_matches_the_tauri_config() {
        let config: Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");
        assert_eq!(config["identifier"], IDENTIFIER);
        assert!(
            config["app"].get("appDirectoriesOverride").is_none(),
            "the MCP mode assumes the default data directory"
        );
    }
}
