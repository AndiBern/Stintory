// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Names for the numeric ids in the packets.
//!
//! The spec defines these in an appendix that we could not read from EA
//! directly. The values below are **not yet verified** against the official
//! appendix or our own recordings (docs/games/f1.md). Only names depend on
//! them: track and car keys use the numeric ids, so fixing a name later
//! changes nothing stored.

use sy_model::SessionType;

/// Human-readable track name for `trackId`.
pub fn track_name(id: i8) -> String {
    let name = match id {
        0 => "Melbourne",
        1 => "Paul Ricard",
        2 => "Shanghai",
        3 => "Sakhir (Bahrain)",
        4 => "Catalunya",
        5 => "Monaco",
        6 => "Montreal",
        7 => "Silverstone",
        8 => "Hockenheim",
        9 => "Hungaroring",
        10 => "Spa",
        11 => "Monza",
        12 => "Singapore",
        13 => "Suzuka",
        14 => "Abu Dhabi",
        15 => "Texas",
        16 => "Brazil",
        17 => "Austria",
        18 => "Sochi",
        19 => "Mexico",
        20 => "Baku (Azerbaijan)",
        21 => "Sakhir Short",
        22 => "Silverstone Short",
        23 => "Texas Short",
        24 => "Suzuka Short",
        25 => "Hanoi",
        26 => "Zandvoort",
        27 => "Imola",
        28 => "Portimão",
        29 => "Jeddah",
        30 => "Miami",
        31 => "Las Vegas",
        32 => "Losail",
        39 => "Silverstone (Reverse)",
        40 => "Austria (Reverse)",
        41 => "Zandvoort (Reverse)",
        42 => "Madrid",
        _ => return format!("Track {id}"),
    };
    name.to_owned()
}

/// Session type for `sessionType`.
pub fn session_type(v: u8) -> SessionType {
    match v {
        1..=4 => SessionType::Practice,
        // qualifying, one-shot qualifying, sprint shootouts
        5..=14 => SessionType::Qualifying,
        15..=17 => SessionType::Race,
        18 => SessionType::TimeTrial,
        _ => SessionType::Other,
    }
}

/// Human-readable session type, e.g. "Qualifying 2".
pub fn session_type_name(v: u8) -> &'static str {
    match v {
        1 => "Practice 1",
        2 => "Practice 2",
        3 => "Practice 3",
        4 => "Short practice",
        5 => "Qualifying 1",
        6 => "Qualifying 2",
        7 => "Qualifying 3",
        8 => "Short qualifying",
        9 => "One-shot qualifying",
        10 => "Sprint shootout 1",
        11 => "Sprint shootout 2",
        12 => "Sprint shootout 3",
        13 => "Short sprint shootout",
        14 => "One-shot sprint shootout",
        15 => "Race",
        16 => "Race 2",
        17 => "Race 3",
        18 => "Time trial",
        _ => "Unknown",
    }
}

/// Weather for `weather`.
pub fn weather(v: u8) -> Option<&'static str> {
    Some(match v {
        0 => "Clear",
        1 => "Light cloud",
        2 => "Overcast",
        3 => "Light rain",
        4 => "Heavy rain",
        5 => "Storm",
        _ => return None,
    })
}

/// Formula for `formula` (from the spec).
pub fn formula(v: u8) -> &'static str {
    match v {
        0 => "F1",
        1 => "F1 Classic",
        2 => "F2",
        3 => "F1 Generic",
        4 => "Beta",
        6 => "Esports",
        8 => "F1 World",
        9 => "F1 Elimination",
        13 => "F1 26",
        _ => "Unknown formula",
    }
}

/// Team name and the car's series for a formula and team id. Confirmed with
/// recordings: F1 8 McLaren; F1 26 476 Mercedes; F2 160 Rodin Motorsport
/// (2024), 466 Campos Racing (2025), 499 Invicta Racing (2026); F1 World 41.
/// The other F1 teams 0–9 are the game's long-standing order, not yet
/// checked. Ids are per car of a season, so they are not shared between
/// formulas.
fn team(formula_id: u8, team_id: u16) -> Option<(&'static str, &'static str)> {
    Some(match (formula_id, team_id) {
        (0, 0) => ("Mercedes", "F1 2025"),
        (0, 1) => ("Ferrari", "F1 2025"),
        (0, 2) => ("Red Bull Racing", "F1 2025"),
        (0, 3) => ("Williams", "F1 2025"),
        (0, 4) => ("Aston Martin", "F1 2025"),
        (0, 5) => ("Alpine", "F1 2025"),
        (0, 6) => ("Racing Bulls", "F1 2025"),
        (0, 7) => ("Haas", "F1 2025"),
        (0, 8) => ("McLaren", "F1 2025"),
        (0, 9) => ("Sauber", "F1 2025"),
        (13, 476) => ("Mercedes", "F1 26"),
        (2, 160) => ("Rodin Motorsport", "F2 2024"),
        (2, 466) => ("Campos Racing", "F2 2025"),
        (2, 499) => ("Invicta Racing", "F2 2026"),
        (8, 41) => ("F1 World car", "F1 World"),
        _ => return None,
    })
}

/// Human-readable car name, e.g. "McLaren (F1 2025)" or "team 12 (F2)".
pub fn car_name(formula_id: u8, team_id: Option<u16>) -> String {
    match team_id {
        Some(t) => match team(formula_id, t) {
            Some((name, series)) if name.starts_with(series) => name.to_owned(),
            Some((name, series)) => format!("{name} ({series})"),
            None => format!("team {t} ({})", formula(formula_id)),
        },
        None => formula(formula_id).to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_fallbacks() {
        assert_eq!(track_name(10), "Spa");
        assert_eq!(track_name(-1), "Track -1");
        assert_eq!(session_type(18), SessionType::TimeTrial);
        assert_eq!(session_type(15), SessionType::Race);
        assert_eq!(session_type(10), SessionType::Qualifying);
        assert_eq!(session_type(0), SessionType::Other);
        assert_eq!(car_name(0, Some(8)), "McLaren (F1 2025)");
        assert_eq!(car_name(13, Some(476)), "Mercedes (F1 26)");
        assert_eq!(car_name(2, Some(499)), "Invicta Racing (F2 2026)");
        assert_eq!(car_name(8, Some(41)), "F1 World car");
        assert_eq!(car_name(2, Some(8)), "team 8 (F2)");
        assert_eq!(car_name(0, Some(40)), "team 40 (F1)");
        assert_eq!(car_name(2, None), "F2");
    }
}
