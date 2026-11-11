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

/// Human-readable car name. Team names are not known yet (the appendix is
/// missing), so the team is shown by number.
pub fn car_name(formula_id: u8, team_id: Option<u16>) -> String {
    match team_id {
        Some(t) => format!("{} · team {t}", formula(formula_id)),
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
        assert_eq!(car_name(0, Some(3)), "F1 · team 3");
        assert_eq!(car_name(2, None), "F2");
    }
}
