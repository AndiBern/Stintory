// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Data transfer types of the frontend API, shared by the desktop backend
//! (Tauri commands) and sy-server (`/api/v1`). Serialized in camelCase.
//!
//! The TypeScript counterparts live in `web/src/lib/api/types.ts`; keep both
//! in sync.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::game::{GameId, SessionType};
use crate::ids::{LapId, SessionId};
use crate::session::{Conditions, Lap, Origin};

/// Which backend serves the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// The desktop app
    Desktop,
    /// sy-server
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
    /// Track name
    pub track_name: String,
    /// Canonical car key
    pub car_key: String,
    /// Car name
    pub car_name: String,
    /// Session type
    pub session_type: SessionType,
    /// Start time
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    /// Number of laps
    pub lap_count: u32,
    /// Best valid lap time in ms, if any
    pub best_lap_ms: Option<u32>,
    /// Where the session came from
    pub origin: OriginView,
    /// Others can see the session (server only; a single shared lap doesn't
    /// count)
    #[serde(default)]
    pub shared: bool,
}

/// Where a session came from ([`Origin`] in camelCase).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OriginView {
    /// Recorded by us
    Own,
    /// Imported from an `.stintory` file
    Imported {
        /// Driver name from the file
        driver_name: Option<String>,
    },
    /// Shared with us on a server
    Synced {
        /// Server base URL
        server: String,
        /// Owner's username
        owner: String,
    },
}

impl From<Origin> for OriginView {
    fn from(o: Origin) -> Self {
        match o {
            Origin::Own => Self::Own,
            Origin::Imported { driver_name } => Self::Imported { driver_name },
            Origin::Synced { server, owner } => Self::Synced { server, owner },
        }
    }
}

/// One lap in the session detail view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LapSummary {
    /// Lap id
    pub id: LapId,
    /// Lap number as counted by the game
    pub number: u32,
    /// Lap time in ms (None if incomplete)
    pub lap_time_ms: Option<u32>,
    /// Sector times in ms
    pub sector_times_ms: Vec<u32>,
    /// Validity
    pub valid: bool,
    /// Reason(s) if invalid
    pub invalid_reason: Option<String>,
    /// Lap started in the pits
    pub out_lap: bool,
    /// Lap ended in the pits
    pub in_lap: bool,
    /// Lap not completed
    pub incomplete: bool,
    /// Rewind/flashback during the lap
    pub flashback: bool,
}

impl From<&Lap> for LapSummary {
    fn from(l: &Lap) -> Self {
        Self {
            id: l.id,
            number: l.number,
            lap_time_ms: l.lap_time_ms,
            sector_times_ms: l.sector_times_ms.clone(),
            valid: l.valid,
            invalid_reason: l.invalid_reason.clone(),
            out_lap: l.flags.out_lap,
            in_lap: l.flags.in_lap,
            incomplete: l.flags.incomplete,
            flashback: l.flags.flashback,
        }
    }
}

/// A session with its laps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetail {
    /// Summary fields (as in the sessions list)
    pub summary: SessionSummary,
    /// End time
    #[serde(with = "time::serde::rfc3339::option")]
    pub ended_at: Option<OffsetDateTime>,
    /// Game version
    pub game_version: Option<String>,
    /// Conditions
    pub conditions: ConditionsView,
    /// Laps, ordered by number
    pub laps: Vec<LapSummary>,
}

/// Weather and temperatures.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConditionsView {
    /// Air temperature, °C
    pub air_temp_c: Option<f32>,
    /// Track temperature, °C
    pub track_temp_c: Option<f32>,
    /// Weather description
    pub weather: Option<String>,
}

impl From<&Conditions> for ConditionsView {
    fn from(c: &Conditions) -> Self {
        Self {
            air_temp_c: c.air_temp_c,
            track_temp_c: c.track_temp_c,
            weather: c.weather.clone(),
        }
    }
}

/// Request to compare laps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareRequest {
    /// Laps to compare; the first one is the reference for the delta
    pub lap_ids: Vec<LapId>,
    /// Resolution of the distance grid in metres (default 1 m)
    pub step_m: Option<f32>,
}

/// One lap resampled onto the common distance grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LapTrace {
    /// Lap id
    pub lap_id: LapId,
    /// Session of the lap
    pub session_id: SessionId,
    /// Lap number
    pub number: u32,
    /// Lap time in ms
    pub lap_time_ms: Option<u32>,
    /// Validity
    pub valid: bool,
    /// Channel name → values on the grid (`null` where the lap has no data)
    pub channels: BTreeMap<String, Vec<f32>>,
    /// Time difference to the reference lap at each grid point, seconds
    /// (positive = slower than the reference)
    pub delta_s: Vec<f32>,
}

/// Laps resampled onto a common distance grid, ready for charts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareData {
    /// Grid resolution in metres
    pub step_m: f32,
    /// Distance of each grid point from the start/finish line, metres
    pub dist_m: Vec<f32>,
    /// Unit symbol of each channel
    pub units: BTreeMap<String, String>,
    /// The laps, reference first
    pub laps: Vec<LapTrace>,
}

/// Result of importing files.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    /// Number of files read
    pub files: u32,
    /// Sessions found (new or updated)
    pub sessions: u32,
    /// Laps found (already known laps are counted but not stored twice)
    pub laps: u32,
    /// Laps of `.stintory` files that were already stored
    pub skipped_laps: u32,
}

/// What to write into an `.stintory` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    /// Sessions with all their laps
    #[serde(default)]
    pub session_ids: Vec<SessionId>,
    /// Single laps (with their session)
    #[serde(default)]
    pub lap_ids: Vec<LapId>,
    /// Driver name shown to whoever imports the file; None = anonymous
    pub driver_name: Option<String>,
    /// Include car setups
    #[serde(default)]
    pub include_setup: bool,
}

/// Result of an export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    /// Written file, or directory for several CSV files
    pub path: String,
    /// Sessions written
    pub sessions: u32,
    /// Laps written
    pub laps: u32,
}

/// Import progress, sent as event while importing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportProgress {
    /// Fraction done, 0–1
    pub fraction: f32,
    /// Laps found so far
    pub laps: u32,
}

/// Client settings shown in the settings page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Where the data is stored
    pub data_dir: String,
    /// Debug: keep raw recordings after processing
    pub keep_raw_recordings: bool,
    /// Size of all raw recordings on disk, bytes
    pub raw_storage_bytes: u64,
    /// Raw recordings that could not be processed
    pub failed_recordings: u32,
    /// Driver name used for the last export (pre-filled in the dialog)
    pub export_driver_name: Option<String>,
}

/// State of the live recorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecorderState {
    /// Live recording is not available on this platform
    Unsupported,
    /// Waiting for a supported game to start
    Waiting,
    /// Recording
    Recording,
}

/// Status of the live recorder, sent as event when it changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecorderStatus {
    /// State
    pub state: RecorderState,
    /// Game being recorded
    pub game: Option<GameId>,
    /// Track of the current session
    pub track_name: Option<String>,
    /// Car of the current session
    pub car_name: Option<String>,
    /// Laps stored in the current session
    pub laps: u32,
    /// Last stored lap time
    pub last_lap_ms: Option<u32>,
}

impl RecorderStatus {
    /// Status for a given state with no session.
    pub fn idle(state: RecorderState) -> Self {
        Self {
            state,
            game: None,
            track_name: None,
            car_name: None,
            laps: 0,
            last_lap_ms: None,
        }
    }
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
            track_name: "Spa".into(),
            car_key: "f1_25:0/13".into(),
            car_name: "Mercedes".into(),
            session_type: SessionType::TimeTrial,
            started_at: datetime!(2026-11-10 18:00:00 UTC),
            lap_count: 12,
            best_lap_ms: Some(83_456),
            origin: OriginView::Imported {
                driver_name: Some("Max".into()),
            },
            shared: false,
        };
        let v = serde_json::to_value(&s).unwrap();
        for key in [
            "id",
            "game",
            "trackKey",
            "carKey",
            "trackName",
            "carName",
            "sessionType",
            "startedAt",
            "lapCount",
            "bestLapMs",
            "origin",
        ] {
            assert!(v.get(key).is_some(), "missing {key} in {v}");
        }
        assert_eq!(v["game"], "f1_25");
        assert_eq!(
            v["origin"],
            serde_json::json!({"kind": "imported", "driverName": "Max"})
        );
    }
}

/// What the desktop app's sync with a server is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    /// Not connected to a server
    Disconnected,
    /// Waiting for the login in the browser
    Connecting,
    /// Connected, nothing running
    Idle,
    /// A sync is running
    Syncing,
}

/// Sync status shown in the desktop app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    /// What's happening
    pub state: SyncState,
    /// The server, when connected
    pub server_url: Option<String>,
    /// The account on the server
    pub username: Option<String>,
    /// This device's name on the server
    pub device_name: Option<String>,
    /// Sync automatically after recording and every few minutes
    pub auto_sync: bool,
    /// The device token is in the system keychain (false: in the app's
    /// data folder, because no keychain is available)
    pub token_in_keychain: bool,
    /// End of the last successful sync
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_sync_at: Option<OffsetDateTime>,
    /// Sessions + laps sent in the last sync
    pub last_pushed: u32,
    /// Sessions + laps received in the last sync
    pub last_pulled: u32,
    /// Why the last sync or connection attempt failed
    pub last_error: Option<String>,
}

impl SyncStatus {
    /// Not connected.
    pub fn disconnected() -> Self {
        Self {
            state: SyncState::Disconnected,
            server_url: None,
            username: None,
            device_name: None,
            auto_sync: true,
            token_in_keychain: false,
            last_sync_at: None,
            last_pushed: 0,
            last_pulled: 0,
            last_error: None,
        }
    }
}
