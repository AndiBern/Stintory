// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Supported games and session types.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A supported game. The string form (`as_str`) is stable and used in
/// databases, file names and track/car keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameId {
    /// iRacing
    Iracing,
    /// EA SPORTS F1 25 (including the 2026 Season Pack)
    #[serde(rename = "f1_25")]
    F1_25,
    /// Le Mans Ultimate
    Lmu,
}

impl GameId {
    /// All supported games.
    pub const ALL: [GameId; 3] = [GameId::Iracing, GameId::F1_25, GameId::Lmu];

    /// Stable machine-readable name.
    pub const fn as_str(self) -> &'static str {
        match self {
            GameId::Iracing => "iracing",
            GameId::F1_25 => "f1_25",
            GameId::Lmu => "lmu",
        }
    }

    /// Human-readable name.
    pub const fn display_name(self) -> &'static str {
        match self {
            GameId::Iracing => "iRacing",
            GameId::F1_25 => "F1 25",
            GameId::Lmu => "Le Mans Ultimate",
        }
    }
}

impl fmt::Display for GameId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error for unknown game names.
#[derive(Debug, thiserror::Error)]
#[error("unknown game: {0}")]
pub struct UnknownGame(String);

impl FromStr for GameId {
    type Err = UnknownGame;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        GameId::ALL
            .into_iter()
            .find(|g| g.as_str() == s)
            .ok_or_else(|| UnknownGame(s.to_owned()))
    }
}

/// Kind of session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionType {
    /// Practice / open test
    Practice,
    /// Qualifying
    Qualifying,
    /// Race
    Race,
    /// Time trial / hotlap mode
    TimeTrial,
    /// Anything we can't classify
    Other,
}

impl SessionType {
    /// Stable machine-readable name.
    pub const fn as_str(self) -> &'static str {
        match self {
            SessionType::Practice => "practice",
            SessionType::Qualifying => "qualifying",
            SessionType::Race => "race",
            SessionType::TimeTrial => "time_trial",
            SessionType::Other => "other",
        }
    }
}

impl fmt::Display for SessionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SessionType {
    type Err = UnknownSessionType;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        [
            SessionType::Practice,
            SessionType::Qualifying,
            SessionType::Race,
            SessionType::TimeTrial,
            SessionType::Other,
        ]
        .into_iter()
        .find(|t| t.as_str() == s)
        .ok_or_else(|| UnknownSessionType(s.to_owned()))
    }
}

/// Error for unknown session type names.
#[derive(Debug, thiserror::Error)]
#[error("unknown session type: {0}")]
pub struct UnknownSessionType(String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_string_form_matches_serde() {
        for game in GameId::ALL {
            let json = serde_json::to_string(&game).unwrap();
            assert_eq!(json, format!("\"{}\"", game.as_str()));
            assert_eq!(game.as_str().parse::<GameId>().unwrap(), game);
        }
    }

    #[test]
    fn unknown_game_is_rejected() {
        assert!("forza".parse::<GameId>().is_err());
    }

    #[test]
    fn session_type_roundtrip() {
        let t: SessionType = "time_trial".parse().unwrap();
        assert_eq!(t, SessionType::TimeTrial);
        assert_eq!(serde_json::to_string(&t).unwrap(), "\"time_trial\"");
    }
}
