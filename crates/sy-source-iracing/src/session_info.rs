// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Player-only session info (docs/PLAN.md §5.2).
//!
//! iRacing's session info YAML contains every driver in the session (names,
//! ids, ratings, results). [`SessionInfo::from_yaml`] builds a new structure
//! from an explicit **whitelist** of fields: track, weather, event, the list
//! of sessions (without results), the player's own driver entry and setup.
//! Nothing about other drivers is ever copied. This filtered structure is
//! what gets spooled and stored.

use serde::{Deserialize, Serialize};

use crate::yaml::{self, Yaml};

/// Track information (`WeekendInfo`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackInfo {
    /// iRacing track id (unique per configuration)
    pub id: Option<i64>,
    /// Internal name, e.g. `snetterton 300`
    pub name: Option<String>,
    /// Display name, e.g. `Snetterton Racing Circuit`
    pub display_name: Option<String>,
    /// Configuration, e.g. `300 Circuit`
    pub config_name: Option<String>,
    /// Track length in metres
    pub length_m: Option<f64>,
    /// Reference latitude (degrees), origin for local coordinates
    pub latitude_deg: Option<f64>,
    /// Reference longitude (degrees)
    pub longitude_deg: Option<f64>,
    /// Number of turns
    pub num_turns: Option<i64>,
    /// Sector start positions as fraction of the lap (`SplitTimeInfo`)
    pub sector_start_pct: Vec<f64>,
}

/// Weather at the time of the session info update.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WeatherInfo {
    /// Air temperature, °C
    pub air_temp_c: Option<f64>,
    /// Track surface temperature, °C
    pub track_temp_c: Option<f64>,
    /// Sky description, e.g. `Partly Cloudy`
    pub skies: Option<String>,
    /// Relative humidity, %
    pub humidity_pct: Option<f64>,
    /// Precipitation, %
    pub precipitation_pct: Option<f64>,
}

/// Event information.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EventInfo {
    /// Session id (0 for offline sessions)
    pub session_id: Option<i64>,
    /// Sub session id (0 for offline sessions)
    pub sub_session_id: Option<i64>,
    /// Series id
    pub series_id: Option<i64>,
    /// Season id
    pub season_id: Option<i64>,
    /// Official session
    pub official: Option<bool>,
    /// Event type, e.g. `Race`, `Test`
    pub event_type: Option<String>,
    /// Category, e.g. `SportsCar`
    pub category: Option<String>,
    /// Sim build version
    pub build_version: Option<String>,
}

/// One session of the event (practice, qualifying, race).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionDef {
    /// Session number (`SessionNum` telemetry variable)
    pub num: i64,
    /// iRacing session type, e.g. `Practice`, `Lone Qualify`, `Race`
    pub session_type: Option<String>,
    /// Session name, e.g. `PRACTICE`
    pub name: Option<String>,
}

/// The player's own driver and car.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayerInfo {
    /// Car index of the player
    pub car_idx: Option<i64>,
    /// iRacing customer id of the player
    pub user_id: Option<i64>,
    /// Player name
    pub user_name: Option<String>,
    /// iRacing car id
    pub car_id: Option<i64>,
    /// Car path, e.g. `raygr22`
    pub car_path: Option<String>,
    /// Car display name
    pub car_screen_name: Option<String>,
    /// Car class id
    pub car_class_id: Option<i64>,
    /// Setup file name
    pub setup_name: Option<String>,
}

/// Session info filtered to the player (see module docs).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    /// Wall-clock time of session time 0, Unix seconds
    pub session_epoch_unix: f64,
    /// Track
    pub track: TrackInfo,
    /// Weather
    pub weather: WeatherInfo,
    /// Event
    pub event: EventInfo,
    /// Sessions of the event (no results)
    pub sessions: Vec<SessionDef>,
    /// The player
    pub player: PlayerInfo,
    /// The player's car setup (iRacing structure, values as text)
    pub setup: Option<serde_json::Value>,
}

impl SessionInfo {
    /// Builds the player-only info from iRacing's YAML text.
    /// `session_epoch_unix` is the wall-clock time of session time 0.
    pub fn from_yaml(text: &str, session_epoch_unix: f64) -> Self {
        let y = yaml::parse(text);
        let s = |p: &str| y.path(p).and_then(Yaml::as_str).map(str::to_owned);
        let f = |p: &str| y.path(p).and_then(Yaml::as_f64);
        let i = |p: &str| y.path(p).and_then(Yaml::as_i64);

        let player_idx = i("DriverInfo.DriverCarIdx");
        let driver = y
            .path("DriverInfo.Drivers")
            .map(Yaml::items)
            .unwrap_or_default()
            .iter()
            .find(|d| d.get("CarIdx").and_then(Yaml::as_i64) == player_idx);
        let ds = |k: &str| {
            driver
                .and_then(|d| d.get(k))
                .and_then(Yaml::as_str)
                .map(str::to_owned)
        };
        let di = |k: &str| driver.and_then(|d| d.get(k)).and_then(Yaml::as_i64);

        Self {
            session_epoch_unix,
            track: TrackInfo {
                id: i("WeekendInfo.TrackID"),
                name: s("WeekendInfo.TrackName"),
                display_name: s("WeekendInfo.TrackDisplayName"),
                config_name: s("WeekendInfo.TrackConfigName"),
                length_m: f("WeekendInfo.TrackLength").map(|km| km * 1000.0),
                latitude_deg: f("WeekendInfo.TrackLatitude"),
                longitude_deg: f("WeekendInfo.TrackLongitude"),
                num_turns: i("WeekendInfo.TrackNumTurns"),
                sector_start_pct: y
                    .path("SplitTimeInfo.Sectors")
                    .map(Yaml::items)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|sec| sec.get("SectorStartPct").and_then(Yaml::as_f64))
                    .collect(),
            },
            weather: WeatherInfo {
                air_temp_c: f("WeekendInfo.TrackAirTemp"),
                track_temp_c: f("WeekendInfo.TrackSurfaceTemp"),
                skies: s("WeekendInfo.TrackSkies"),
                humidity_pct: f("WeekendInfo.TrackRelativeHumidity"),
                precipitation_pct: f("WeekendInfo.TrackPrecipitation"),
            },
            event: EventInfo {
                session_id: i("WeekendInfo.SessionID"),
                sub_session_id: i("WeekendInfo.SubSessionID"),
                series_id: i("WeekendInfo.SeriesID"),
                season_id: i("WeekendInfo.SeasonID"),
                official: i("WeekendInfo.Official").map(|v| v != 0),
                event_type: s("WeekendInfo.EventType"),
                category: s("WeekendInfo.Category"),
                build_version: s("WeekendInfo.BuildVersion"),
            },
            sessions: y
                .path("SessionInfo.Sessions")
                .map(Yaml::items)
                .unwrap_or_default()
                .iter()
                .filter_map(|sess| {
                    Some(SessionDef {
                        num: sess.get("SessionNum")?.as_i64()?,
                        session_type: sess
                            .get("SessionType")
                            .and_then(Yaml::as_str)
                            .map(str::to_owned),
                        name: sess
                            .get("SessionName")
                            .and_then(Yaml::as_str)
                            .map(str::to_owned),
                    })
                })
                .collect(),
            player: PlayerInfo {
                car_idx: player_idx,
                user_id: i("DriverInfo.DriverUserID").or_else(|| di("UserID")),
                user_name: ds("UserName"),
                car_id: di("CarID"),
                car_path: ds("CarPath"),
                car_screen_name: ds("CarScreenName"),
                car_class_id: di("CarClassID"),
                setup_name: s("DriverInfo.DriverSetupName"),
            },
            setup: y.get("CarSetup").map(Yaml::to_json),
        }
    }

    /// Session definition for a session number.
    pub fn session(&self, num: i64) -> Option<&SessionDef> {
        self.sessions.iter().find(|s| s.num == num)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML: &str = "---
WeekendInfo:
 TrackName: oschersleben gp
 TrackID: 449
 TrackLength: 3.6100 km
 TrackDisplayName: Motorsport Arena Oschersleben
 TrackConfigName: Grand Prix
 TrackLatitude: 52.027115 m
 TrackLongitude: 11.279532 m
 TrackSurfaceTemp: 24.70 C
 TrackAirTemp: 18.70 C
 TrackSkies: Partly Cloudy
 SessionID: 1
 SubSessionID: 2
 Official: 1
SessionInfo:
 Sessions:
 - SessionNum: 0
   SessionType: Practice
   ResultsPositions:
   - Position: 1
     CarIdx: 3
 - SessionNum: 2
   SessionType: Race
SplitTimeInfo:
 Sectors:
 - SectorNum: 0
   SectorStartPct: 0.000000
 - SectorNum: 1
   SectorStartPct: 0.362077
CarSetup:
 Tires:
  LeftFront:
   ColdPressure: 124 kPa
DriverInfo:
 DriverCarIdx: 7
 DriverUserID: 4242
 DriverSetupName: fixed.sto
 Drivers:
 - CarIdx: 3
   UserName: Other Driver
   UserID: 999
   IRating: 7737
   CarID: 163
 - CarIdx: 7
   UserName: Me Myself
   UserID: 4242
   IRating: 1500
   CarID: 163
   CarPath: raygr22
   CarScreenName: Ray Formula 1600
   CarClassID: 4016
";

    #[test]
    fn extracts_player_and_session_data() {
        let info = SessionInfo::from_yaml(YAML, 1_794_230_824.0);
        assert_eq!(info.track.id, Some(449));
        assert_eq!(info.track.length_m, Some(3610.0));
        assert_eq!(info.track.sector_start_pct, vec![0.0, 0.362077]);
        assert_eq!(info.weather.air_temp_c, Some(18.7));
        assert_eq!(info.player.car_idx, Some(7));
        assert_eq!(info.player.user_name.as_deref(), Some("Me Myself"));
        assert_eq!(info.player.car_id, Some(163));
        assert_eq!(
            info.session(2).unwrap().session_type.as_deref(),
            Some("Race")
        );
        assert_eq!(
            info.setup.as_ref().unwrap()["Tires"]["LeftFront"]["ColdPressure"],
            "124 kPa"
        );
    }

    #[test]
    fn never_contains_other_drivers() {
        let info = SessionInfo::from_yaml(YAML, 0.0);
        let json = serde_json::to_string(&info).unwrap();
        assert!(!json.contains("Other Driver"), "{json}");
        assert!(!json.contains("999"), "other driver's id leaked: {json}");
        assert!(
            !json.contains("7737"),
            "other driver's rating leaked: {json}"
        );
        assert!(
            !json.contains("1500"),
            "even our own rating is not needed: {json}"
        );
        assert!(!json.contains("Position"), "results leaked: {json}");
    }
}
