// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tauri commands called by the frontend (`web/src/lib/api/tauri.ts`).

use rz_model::api::{AppInfo, BackendKind, SessionSummary};
use serde::Serialize;
use tauri::State;

use crate::state::AppState;

/// Error returned to the frontend. Serialized as a plain message string.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct CommandError(#[from] rz_store::StoreError);

impl Serialize for CommandError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// Basic information about the backend.
#[tauri::command]
pub fn get_info() -> AppInfo {
    AppInfo {
        name: "Rundenzeit".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        backend: BackendKind::Desktop,
    }
}

/// All recorded sessions, newest first.
#[tauri::command]
pub async fn list_sessions(
    state: State<'_, AppState>,
) -> Result<Vec<SessionSummary>, CommandError> {
    Ok(state.with_store(|store| store.list_sessions()).await?)
}
