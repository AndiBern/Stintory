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
use crate::remote::UserRef;
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
    /// Weather, fuel and tyres during the lap, from its telemetry (None
    /// where the telemetry wasn't read)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<LapConditions>,
}

/// Weather, fuel and tyres of one lap, derived from its telemetry. Each
/// value is None if the game doesn't provide it (or didn't when the lap was
/// recorded).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LapConditions {
    /// Air temperature, °C, average over the lap
    pub air_temp_c: Option<f32>,
    /// Track temperature, °C, average over the lap
    pub track_temp_c: Option<f32>,
    /// Relative humidity 0–1, average over the lap
    pub humidity: Option<f32>,
    /// Weather description (game-specific wording)
    pub weather: Option<String>,
    /// Fuel in the tank at the start of the lap
    pub fuel_start: Option<f32>,
    /// Fuel used during the lap
    pub fuel_used: Option<f32>,
    /// Unit of the fuel values: `l` or `kg`
    pub fuel_unit: Option<String>,
    /// Tyre (surface) temperature per wheel, °C, average over the lap
    pub tyre_temp_c: Option<Wheels>,
    /// Tyre wear per wheel at the end of the lap, 0 = new … 1 = worn out
    /// (before a puncture: the game reports a flat tyre as worn out)
    pub tyre_wear: Option<Wheels>,
    /// Tyres that went flat (or wheels that came off) during the lap; None
    /// if none did or the game doesn't say
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tyre_flat: Option<WheelFlags>,
}

impl LapConditions {
    /// Nothing is known.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// One flag per wheel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WheelFlags {
    /// Front left
    pub fl: bool,
    /// Front right
    pub fr: bool,
    /// Rear left
    pub rl: bool,
    /// Rear right
    pub rr: bool,
}

/// One value per wheel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Wheels {
    /// Front left
    pub fl: f32,
    /// Front right
    pub fr: f32,
    /// Rear left
    pub rl: f32,
    /// Rear right
    pub rr: f32,
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
            conditions: None,
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
    /// Who drove the lap, if not the user: the owner of a lap someone
    /// shared, the driver named in an imported file. Set by the backends.
    #[serde(default)]
    pub driver: Option<String>,
    /// The owner of a lap someone shared on the server (server only: the
    /// desktop app keeps such laps as local copies)
    #[serde(default)]
    pub owner: Option<UserRef>,
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

/// A lap to compare with: one of the user's own (recorded or imported) or
/// one someone shared, for a given track and car. Lists of them are sorted
/// by lap time, fastest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceLap {
    /// Lap id
    pub lap_id: LapId,
    /// Session of the lap
    pub session_id: SessionId,
    /// Lap number
    pub number: u32,
    /// Lap time in ms
    pub lap_time_ms: u32,
    /// Sector times in ms
    pub sector_times_ms: Vec<u32>,
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
    /// Session start
    #[serde(with = "time::serde::rfc3339")]
    pub session_started_at: OffsetDateTime,
    /// Where the session came from
    pub origin: OriginView,
    /// The owner, for laps someone shared on the server; None for the
    /// user's own laps
    pub owner: Option<UserRef>,
    /// Desktop app: the lap is only on the server so far. It is kept as a
    /// local copy once chosen (see `ApiClient.prepareLaps` in the frontend)
    #[serde(default)]
    pub remote: bool,
}

/// Which laps a reference list contains.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceScope {
    /// The user's own laps and laps shared with them
    #[default]
    All,
    /// Only the user's own laps (recorded and imported)
    Mine,
    /// Only laps others shared with the user
    Shared,
}

/// Most laps per driver in a reference list.
pub const REFERENCE_LAPS_PER_DRIVER: usize = 10;
/// Most laps in a reference list.
pub const REFERENCE_LAPS_MAX: usize = 200;

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
    /// Recording F1 25 (UDP telemetry)
    #[serde(default)]
    pub f1_capture: F1CaptureSettings,
    /// Where the app listens for F1 packets right now, e.g. `127.0.0.1:20777`
    #[serde(default)]
    pub f1_listening: Option<String>,
    /// Why the app can't listen for F1 packets (e.g. the port is taken)
    #[serde(default)]
    pub f1_error: Option<String>,
    /// Recording Le Mans Ultimate (shared memory)
    #[serde(default)]
    pub lmu_capture: LmuCaptureSettings,
    /// Le Mans Ultimate can be recorded on this platform (Windows)
    #[serde(default)]
    pub lmu_available: bool,
    /// What the app sees of Le Mans Ultimate right now
    #[serde(default)]
    pub lmu_status: Option<String>,
    /// Something to do in the game before it can be recorded (e.g. turn on
    /// *Enable Plugins*)
    #[serde(default)]
    pub lmu_hint: Option<String>,
    /// Games in Proton can be recorded through the Proton bridge (Linux)
    #[serde(default)]
    pub bridge_available: bool,
    /// The Proton bridge
    #[serde(default)]
    pub proton_bridge: ProtonBridgeSettings,
    /// What the app sees of the bridge right now
    #[serde(default)]
    pub bridge_status: Option<String>,
    /// Why the bridge can't be used (e.g. `sy-bridge.exe` is missing)
    #[serde(default)]
    pub bridge_error: Option<String>,
    /// Where `sy-bridge.exe` is
    #[serde(default)]
    pub bridge_exe: Option<String>,
    /// The Steam launch option that starts the bridge with a game
    #[serde(default)]
    pub bridge_launch_option: Option<String>,
}

/// Settings of the Proton bridge (Linux: games running in Proton).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtonBridgeSettings {
    /// Connect to the bridge when a game starts with it
    pub enabled: bool,
    /// TCP port of the bridge on 127.0.0.1 (default 20790)
    pub port: u16,
}

impl Default for ProtonBridgeSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 20790,
        }
    }
}

/// Settings for recording Le Mans Ultimate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LmuCaptureSettings {
    /// Record when the game runs
    pub enabled: bool,
}

impl Default for LmuCaptureSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Settings for recording F1 25 (UDP telemetry).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct F1CaptureSettings {
    /// Listen for the game's packets
    pub enabled: bool,
    /// UDP port (the game's default is 20777)
    pub port: u16,
    /// Also accept packets from other devices (consoles, other PCs);
    /// otherwise only from this PC
    pub allow_remote: bool,
    /// Forward every packet to these addresses (`host:port`), for other
    /// telemetry apps
    pub forward: Vec<String>,
}

impl Default for F1CaptureSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 20777,
            allow_remote: false,
            forward: Vec::new(),
        }
    }
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
    /// Something the user has to do before a running game can be recorded
    /// (e.g. "turn on Enable Plugins in Le Mans Ultimate")
    #[serde(default)]
    pub hint: Option<String>,
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
            hint: None,
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
