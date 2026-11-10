// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Data transfer types of the frontend API, shared by the desktop backend
//! (Tauri commands) and rz-server (`/api/v1`). Serialized in camelCase.
//!
//! The TypeScript counterparts live in `web/src/lib/api/types.ts`; keep both
//! in sync.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::game::{GameId, SessionType};
use crate::ids::SessionId;

/// Which backend serves the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// The desktop app
    Desktop,
    /// rz-server
    Server,
}

/// Basic information about the backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    /// Application name
    pub name: String,
    /// Version of the backend
    pub version: String,
    /// Backend kind
    pub backend: BackendKind,
}

/// One row of the sessions list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    /// Session id
    pub id: SessionId,
    /// Game
    pub game: GameId,
    /// Canonical track key
    pub track_key: String,
    /// Canonical car key
    pub car_key: String,
    /// Session type
    pub session_type: SessionType,
    /// Start time
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    /// Number of laps
    pub lap_count: u32,
    /// Best valid lap time in ms, if any
    pub best_lap_ms: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn session_summary_uses_frontend_field_names() {
        let s = SessionSummary {
            id: SessionId::new(),
            game: GameId::F1_25,
            track_key: "f1_25:10".into(),
            car_key: "f1_25:0/13".into(),
            session_type: SessionType::TimeTrial,
            started_at: datetime!(2026-11-10 18:00:00 UTC),
            lap_count: 12,
            best_lap_ms: Some(83_456),
        };
        let v = serde_json::to_value(&s).unwrap();
        for key in [
            "id",
            "game",
            "trackKey",
            "carKey",
            "sessionType",
            "startedAt",
            "lapCount",
            "bestLapMs",
        ] {
            assert!(v.get(key).is_some(), "missing {key} in {v}");
        }
        assert_eq!(v["game"], "f1_25");
    }
}
