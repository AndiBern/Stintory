// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Names for the numeric ids in the packets, from the appendices of EA's
//! "Data Output from F1 25" (v3, format 2025) and "Data Output from F1 25:
//! 2026 Season Pack" (v1.2, format 2026). Only names depend on them: track
//! and car keys use the numeric ids.

use sy_model::SessionType;

/// Human-readable track name for `trackId`.
pub fn track_name(id: i8) -> String {
    let name = match id {
        0 => "Melbourne",
        2 => "Shanghai",
        3 => "Sakhir (Bahrain)",
        4 => "Catalunya",
        5 => "Monaco",
        6 => "Montreal",
        7 => "Silverstone",
        9 => "Hungaroring",
        10 => "Spa",
        11 => "Monza",
        12 => "Singapore",
        13 => "Suzuka",
        14 => "Abu Dhabi",
        15 => "Texas",
        16 => "Brazil",
        17 => "Austria",
        19 => "Mexico",
        20 => "Baku (Azerbaijan)",
        26 => "Zandvoort",
        27 => "Imola",
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

/// Weather for `weather` (from the session packet's field description).
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

/// Formula for `formula` (from the session packet's field description).
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

/// Game mode for `gameMode`.
pub fn game_mode(v: u8) -> Option<&'static str> {
    Some(match v {
        4 => "Grand Prix '23",
        5 => "Time Trial",
        6 => "Splitscreen",
        7 => "Online Custom",
        15 => "Online Weekly Event",
        17 => "Story Mode (Braking Point)",
        27 => "My Team Career '25",
        28 => "Driver Career '25",
        29 => "Career '25 Online",
        30 => "Challenge Career '25",
        75 => "Story Mode (APXGP)",
        127 => "Benchmark",
        _ => return None,
    })
}

/// Rule set for `ruleSet`.
pub fn rule_set(v: u8) -> Option<&'static str> {
    Some(match v {
        0 => "Practice & Qualifying",
        1 => "Race",
        2 => "Time Trial",
        12 => "Elimination",
        _ => return None,
    })
}

/// Surface under a wheel (`surfaceType` of the car telemetry), for the
/// `f1.surface_*` channels.
pub fn surface(v: u8) -> Option<&'static str> {
    Some(match v {
        0 => "Tarmac",
        1 => "Rumble strip",
        2 => "Concrete",
        3 => "Rock",
        4 => "Gravel",
        5 => "Mud",
        6 => "Sand",
        7 => "Grass",
        8 => "Water",
        9 => "Cobblestone",
        10 => "Metal",
        11 => "Ridged",
        _ => return None,
    })
}

/// Team name and the season of its car, for `teamId`. Ids are unique
/// across formulas. 0–9 are the 2025 cars, the current ones of F1 25
/// (the appendix gives them without a year).
fn team(id: u16) -> Option<(&'static str, Option<u16>)> {
    let (name, season) = match id {
        0 => ("Mercedes", 2025),
        1 => ("Ferrari", 2025),
        2 => ("Red Bull Racing", 2025),
        3 => ("Williams", 2025),
        4 => ("Aston Martin", 2025),
        5 => ("Alpine", 2025),
        6 => ("RB", 2025),
        7 => ("Haas", 2025),
        8 => ("McLaren", 2025),
        9 => ("Sauber", 2025),
        41 => return Some(("F1 Generic", None)),
        104 => return Some(("F1 Custom Team", None)),
        129 => return Some(("Konnersport", None)),
        142 => ("APXGP", 2024),
        154 => ("APXGP", 2025),
        155 => ("Konnersport", 2024),
        158 | 465 | 489 => ("Art GP", f2_season(id)),
        159 | 466 | 490 => ("Campos", f2_season(id)),
        160 | 467 | 491 => ("Rodin Motorsport", f2_season(id)),
        161 | 468 | 492 => ("AIX Racing", f2_season(id)),
        162 | 469 | 493 => ("DAMS", f2_season(id)),
        163 | 470 | 494 => ("Hitech", f2_season(id)),
        164 | 471 | 495 => ("MP Motorsport", f2_season(id)),
        165 | 472 | 496 => ("Prema", f2_season(id)),
        166 | 473 | 497 => ("Trident", f2_season(id)),
        167 | 474 | 498 => ("Van Amersfoort Racing", f2_season(id)),
        168 | 475 | 499 => ("Invicta", f2_season(id)),
        185 => ("Mercedes", 2024),
        186 => ("Ferrari", 2024),
        187 => ("Red Bull Racing", 2024),
        188 => ("Williams", 2024),
        189 => ("Aston Martin", 2024),
        190 => ("Alpine", 2024),
        191 => ("RB", 2024),
        192 => ("Haas", 2024),
        193 => ("McLaren", 2024),
        194 => ("Sauber", 2024),
        476 => ("Mercedes", 2026),
        477 => ("Ferrari", 2026),
        478 => ("Red Bull Racing", 2026),
        479 => ("Williams", 2026),
        480 => ("Aston Martin", 2026),
        481 => ("Alpine", 2026),
        482 => ("RB", 2026),
        483 => ("Haas", 2026),
        484 => ("McLaren", 2026),
        485 => ("Audi", 2026),
        486 => ("Cadillac", 2026),
        _ => return None,
    };
    Some((name, Some(season)))
}

/// Season of an F2 team id (2024: 158–168, 2025: 465–475, 2026: 489–499).
fn f2_season(id: u16) -> u16 {
    if id <= 168 {
        2024
    } else if id <= 475 {
        2025
    } else {
        2026
    }
}

/// Human-readable car name, e.g. "McLaren (F1 2025)", "Mercedes (F1 26)",
/// "Campos (F2 2025)"; unknown teams by number, "team 600 (F2)".
pub fn car_name(formula_id: u8, team_id: Option<u16>) -> String {
    let formula = formula(formula_id);
    match team_id.map(|t| (t, team(t))) {
        // "F1 26" already names the season
        Some((_, Some((name, Some(_))))) if formula_id == 13 => format!("{name} ({formula})"),
        Some((_, Some((name, Some(season))))) => format!("{name} ({formula} {season})"),
        Some((_, Some((name, None)))) => format!("{name} ({formula})"),
        Some((t, None)) => format!("team {t} ({formula})"),
        None => formula.to_owned(),
    }
}

/// The track-limits part of an infringement type, e.g. "corner cutting".
/// `None` for other infringements (collisions, pit lane, …).
pub fn track_limits(infringement: u8) -> Option<&'static str> {
    Some(match infringement {
        7 => "corner cutting (time gained)",
        8 | 9 => "corner cutting (overtake)",
        25 => "corner cutting",
        26 => "running wide",
        // "corner cutting ran wide gained time" minor/significant/extreme
        27..=29 => "track limits (time gained)",
        30 => "wall riding",
        31 => "flashback used",
        32 => "reset to track",
        _ => return None,
    })
}

/// Why the game invalidated a lap: a penalty of type 10–15 ("lap
/// invalidated", with or without the next or previous lap) and its
/// infringement type. `None` for other penalties.
pub fn invalidation_reason(penalty_type: u8, infringement: u8) -> Option<String> {
    if !(10..=15).contains(&penalty_type) {
        return None;
    }
    Some(match track_limits(infringement) {
        Some(reason) => reason.to_owned(),
        // "without reason" penalties
        None if matches!(penalty_type, 12 | 13 | 15) => "invalidated by the game".to_owned(),
        None => format!("invalidated by the game (infringement {infringement})"),
    })
}

/// The laps a lap-invalidating penalty applies to, relative to the lap it
/// occurred on: this lap, this and the next, or this and the previous.
pub fn invalidated_laps(penalty_type: u8) -> &'static [i32] {
    match penalty_type {
        10 | 12 => &[0],
        11 | 13 => &[0, 1],
        14 | 15 => &[0, -1],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_fallbacks() {
        assert_eq!(track_name(10), "Spa");
        assert_eq!(track_name(42), "Madrid");
        assert_eq!(track_name(-1), "Track -1");
        assert_eq!(session_type(18), SessionType::TimeTrial);
        assert_eq!(session_type(15), SessionType::Race);
        assert_eq!(session_type(10), SessionType::Qualifying);
        assert_eq!(session_type(0), SessionType::Other);
        assert_eq!(game_mode(5), Some("Time Trial"));
        assert_eq!(rule_set(2), Some("Time Trial"));
        assert_eq!(surface(7), Some("Grass"));
    }

    #[test]
    fn car_names() {
        // from the recordings
        assert_eq!(car_name(0, Some(8)), "McLaren (F1 2025)");
        assert_eq!(car_name(13, Some(476)), "Mercedes (F1 26)");
        assert_eq!(car_name(2, Some(160)), "Rodin Motorsport (F2 2024)");
        assert_eq!(car_name(2, Some(466)), "Campos (F2 2025)");
        assert_eq!(car_name(2, Some(499)), "Invicta (F2 2026)");
        assert_eq!(car_name(8, Some(41)), "F1 Generic (F1 World)");
        // from the appendix
        assert_eq!(car_name(13, Some(486)), "Cadillac (F1 26)");
        assert_eq!(car_name(0, Some(193)), "McLaren (F1 2024)");
        assert_eq!(car_name(2, Some(600)), "team 600 (F2)");
        assert_eq!(car_name(2, None), "F2");
    }

    #[test]
    fn invalidation_reasons() {
        assert_eq!(invalidation_reason(10, 26).as_deref(), Some("running wide"));
        assert_eq!(
            invalidation_reason(11, 25).as_deref(),
            Some("corner cutting")
        );
        assert_eq!(
            invalidation_reason(12, 0).as_deref(),
            Some("invalidated by the game")
        );
        assert_eq!(invalidation_reason(5, 26), None, "a warning");
        // warnings and other penalties: only their track-limits part
        assert_eq!(track_limits(28), Some("track limits (time gained)"));
        assert_eq!(track_limits(4), None, "a collision");
        assert_eq!(invalidated_laps(11), &[0, 1]);
        assert_eq!(invalidated_laps(14), &[0, -1]);
        assert!(invalidated_laps(4).is_empty());
    }
}
