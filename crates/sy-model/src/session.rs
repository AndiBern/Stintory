// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Sessions and laps (metadata). Lap telemetry itself lives in a separate
//! blob, see [`crate::telemetry`].
//!
//! Sharing settings are added with the server milestone (docs/PLAN.md §9.2).

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::game::{GameId, SessionType};
use crate::ids::{BlobHash, LapId, SessionId};

/// Where a session came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    /// Recorded on this machine (or another device of the same user).
    Own,
    /// Imported from an `.stintory` bundle.
    Imported {
        /// Display name of the driver as stored in the bundle, if any.
        driver_name: Option<String>,
    },
    /// Shared with us by another user on a server.
    Synced {
        /// Server base URL
        server: String,
        /// Username of the owner on that server
        owner: String,
    },
}

/// Weather and temperatures at session start.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Conditions {
    /// Air temperature, °C
    pub air_temp_c: Option<f32>,
    /// Track temperature, °C
    pub track_temp_c: Option<f32>,
    /// Game-specific weather description
    pub weather: Option<String>,
}

/// One driving session (practice, qualifying, race, …) in one car on one track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// Id, generated on the recording client.
    pub id: SessionId,
    /// Game the session was recorded in.
    pub game: GameId,
    /// Game version string, if the game reports it.
    pub game_version: Option<String>,
    /// Canonical track key incl. layout, e.g. `iracing:449`.
    pub track_key: String,
    /// Human-readable track name incl. layout, e.g. `Oschersleben – Grand Prix`.
    pub track_name: String,
    /// Canonical car key, e.g. `iracing:163`.
    pub car_key: String,
    /// Human-readable car name, e.g. `Ray Formula 1600`.
    pub car_name: String,
    /// Kind of session.
    pub session_type: SessionType,
    /// When the session started.
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    /// When the session ended (None while recording or if unknown).
    #[serde(with = "time::serde::rfc3339::option")]
    pub ended_at: Option<OffsetDateTime>,
    /// Conditions at session start.
    pub conditions: Conditions,
    /// Car setup as reported by the game (game-specific JSON).
    pub setup: Option<serde_json::Value>,
    /// Further session information from the game, filtered to the player
    /// only (never contains other drivers' data, see docs/PLAN.md §5.2).
    pub session_info: Option<serde_json::Value>,
    /// Where the session came from.
    pub origin: Origin,
}

/// Flags describing special laps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LapFlags {
    /// Lap started in the pits.
    pub out_lap: bool,
    /// Lap ended in the pits.
    pub in_lap: bool,
    /// Lap was not completed (session end, tow, disconnect).
    pub incomplete: bool,
    /// A rewind/flashback happened during the lap.
    pub flashback: bool,
}

/// One lap of a session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lap {
    /// Id, generated on the recording client.
    pub id: LapId,
    /// Session this lap belongs to.
    pub session_id: SessionId,
    /// Lap number as counted by the game.
    pub number: u32,
    /// Official lap time in milliseconds (None for incomplete laps).
    pub lap_time_ms: Option<u32>,
    /// Official sector times in milliseconds.
    pub sector_times_ms: Vec<u32>,
    /// Whether the game (or our rules for games without a flag) considers the lap valid.
    pub valid: bool,
    /// Why the lap is invalid, if it is.
    pub invalid_reason: Option<String>,
    /// Special lap flags.
    pub flags: LapFlags,
    /// Hash of the telemetry blob (Parquet file).
    pub blob_hash: BlobHash,
    /// Nominal telemetry sample rate.
    pub sample_rate_hz: f32,
    /// Number of telemetry samples.
    pub sample_count: u32,
    /// Game session time (s) of the lap's first sample, if known.
    #[serde(default)]
    pub start_time_s: Option<f64>,
}

/// A lap before its telemetry is stored: everything of [`Lap`] except the
/// fields derived from the telemetry blob.
#[derive(Debug, Clone, PartialEq)]
pub struct NewLap {
    /// Lap id (generate with [`LapId::new`])
    pub id: LapId,
    /// Session the lap belongs to
    pub session_id: SessionId,
    /// Lap number as counted by the game
    pub number: u32,
    /// Game session time (s) of the lap's first sample; finds the same lap
    /// recorded twice (live and from a game file)
    pub start_time_s: Option<f64>,
    /// Official lap time, None if incomplete
    pub lap_time_ms: Option<u32>,
    /// Official sector times
    pub sector_times_ms: Vec<u32>,
    /// Lap validity
    pub valid: bool,
    /// Reason if invalid
    pub invalid_reason: Option<String>,
    /// Special lap flags
    pub flags: LapFlags,
}

impl NewLap {
    /// Completes the lap with the fields derived from its stored telemetry.
    pub fn into_lap(self, blob_hash: BlobHash, sample_rate_hz: f32, sample_count: u32) -> Lap {
        Lap {
            id: self.id,
            session_id: self.session_id,
            number: self.number,
            lap_time_ms: self.lap_time_ms,
            sector_times_ms: self.sector_times_ms,
            valid: self.valid,
            invalid_reason: self.invalid_reason,
            flags: self.flags,
            blob_hash,
            sample_rate_hz,
            sample_count,
            start_time_s: self.start_time_s,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn session_json_roundtrip() {
        let session = Session {
            id: SessionId::new(),
            game: GameId::Iracing,
            game_version: None,
            track_key: "iracing:261".into(),
            track_name: "Spa – Grand Prix".into(),
            car_key: "iracing:132".into(),
            car_name: "Porsche 911 GT3 R".into(),
            session_type: SessionType::Practice,
            started_at: datetime!(2026-11-10 18:00:00 UTC),
            ended_at: None,
            conditions: Conditions::default(),
            setup: None,
            session_info: None,
            origin: Origin::Imported {
                driver_name: Some("Andi".into()),
            },
        };
        let json = serde_json::to_string(&session).unwrap();
        assert!(
            json.contains("\"started_at\":\"2026-11-10T18:00:00Z\""),
            "{json}"
        );
        assert!(json.contains("\"kind\":\"imported\""), "{json}");
        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(back, session);
    }

    #[test]
    fn lap_flags_default_when_missing() {
        let flags: LapFlags = serde_json::from_str(r#"{"out_lap":true}"#).unwrap();
        assert!(flags.out_lap);
        assert!(!flags.in_lap && !flags.incomplete && !flags.flashback);
    }
}
