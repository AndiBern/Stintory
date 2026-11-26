// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Car setups as lists of settings, so the setups of compared laps can be
//! put side by side.
//!
//! Each game stores its own JSON (docs/games/): iRacing the `CarSetup` tree
//! of the session info (values as the garage shows them), F1 25 the car
//! setup packet with numbers, Le Mans Ultimate the garage settings by key
//! (`VM_*` for the car, `WM_*-W_FL` for a wheel). Values the game measures
//! rather than the driver sets (iRacing's last hot pressures, tire
//! temperatures and wear) are left out.

use serde_json::Value;
use sy_model::GameId;
use sy_model::api::{LapSetup, SetupItem, SetupSource};

/// The setup of a lap for the comparison: the lap's own, else (laps
/// recorded before setups were kept per lap) the session's.
pub fn lap_setup(game: GameId, lap: Option<&Value>, session: Option<&Value>) -> Option<LapSetup> {
    let (source, setup) = match (lap, session) {
        (Some(s), _) => (SetupSource::Lap, s),
        (None, Some(s)) => (SetupSource::Session, s),
        (None, None) => return None,
    };
    let items = setup_items(game, setup);
    (!items.is_empty()).then_some(LapSetup { source, items })
}

/// The session prescribed the setup: `fixed_setup` at the top of the
/// session info (iRacing `WeekendOptions.IsFixedSetup`, Le Mans Ultimate
/// `mIsFixedSetup`; F1 25 doesn't say). False if unknown.
pub fn fixed_setup(session_info: Option<&Value>) -> bool {
    session_info
        .and_then(|i| i.get("fixed_setup"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The session info a session shares with others who see its laps: only
/// whether the setup was fixed (never the rest, docs/ARCHITECTURE.md §9.2).
pub fn shared_session_info(session_info: Option<&Value>) -> Option<Value> {
    session_info
        .and_then(|i| i.get("fixed_setup"))
        .filter(|v| v.is_boolean())
        .map(|v| serde_json::json!({ "fixed_setup": v }))
}

/// The settings of a setup, grouped, in a stable order.
pub fn setup_items(game: GameId, setup: &Value) -> Vec<SetupItem> {
    let mut items = match game {
        GameId::Iracing => iracing(setup),
        GameId::F1_25 => f1(setup),
        GameId::Lmu => lmu(setup),
    };
    // sections in a fixed order, the settings in each as they came
    items.sort_by_key(|(rank, _)| *rank);
    items.into_iter().map(|(_, item)| item).collect()
}

fn item(group: impl Into<String>, name: impl Into<String>, value: impl Into<String>) -> SetupItem {
    SetupItem {
        group: group.into(),
        name: name.into(),
        value: value.into(),
    }
}

/// A JSON value as text: strings as they are, numbers without float noise.
fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.trim().to_owned()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(match n.as_f64() {
            Some(f) if n.is_f64() => {
                let s = format!("{f:.3}");
                s.trim_end_matches('0').trim_end_matches('.').to_owned()
            }
            _ => n.to_string(),
        }),
        Value::Bool(b) => Some(if *b { "on" } else { "off" }.to_owned()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

/// `LeftFront` → `Left front`, `ARBPreload` → `ARB preload`.
fn words(camel: &str) -> String {
    let chars: Vec<char> = camel.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| chars[j]);
        let next = chars.get(i + 1);
        let starts_word = i > 0
            && c.is_uppercase()
            && (prev.is_some_and(|p| p.is_lowercase() || p.is_ascii_digit())
                || (prev.is_some_and(char::is_uppercase)
                    && next.is_some_and(|n| n.is_lowercase())));
        if starts_word {
            out.push(' ');
        }
        out.push(c);
    }
    // lower-case every word but the first, except acronyms (ARB, TC)
    const ACRONYMS: &[&str] = &["Arb", "Farb", "Rarb", "Abs", "Tc"];
    let mut result = Vec::new();
    for (i, w) in out.split(' ').enumerate() {
        let acronym = w.len() > 1 && w.chars().all(|c| c.is_uppercase() || c.is_ascii_digit());
        if acronym || ACRONYMS.contains(&w) {
            result.push(w.to_uppercase());
        } else if i == 0 {
            result.push(w.to_owned());
        } else {
            result.push(w.to_lowercase());
        }
    }
    result.join(" ")
}

// ----- iRacing --------------------------------------------------------------

/// Measured by the game, not set by the driver.
fn iracing_measured(key: &str) -> bool {
    key == "UpdateCount" || key == "TreadRemaining" || key.starts_with("Last")
}

/// Corners in the order the garage shows them; other sections after.
fn iracing_rank(section: &str) -> u8 {
    match section {
        "Front" => 0,
        "LeftFront" => 1,
        "RightFront" => 2,
        "LeftRear" => 3,
        "RightRear" => 4,
        "Rear" => 5,
        _ => 6,
    }
}

/// `CarSetup` of the session info: sections (`Tires`, `Chassis`, …) with
/// sub-sections (`LeftFront`, …) and settings.
fn iracing(setup: &Value) -> Vec<(u32, SetupItem)> {
    let mut out = Vec::new();
    let Some(sections) = setup.as_object() else {
        return out;
    };
    for (s, (section, content)) in sections.iter().enumerate() {
        if iracing_measured(section) {
            continue;
        }
        let s = s as u32 * 1000;
        match content {
            Value::Object(entries) => {
                for (key, value) in entries {
                    if iracing_measured(key) {
                        continue;
                    }
                    if let Value::Object(settings) = value {
                        let group = format!("{} · {}", words(section), words(key));
                        let rank = s + u32::from(iracing_rank(key)) * 100;
                        for (name, v) in settings {
                            if !iracing_measured(name)
                                && let Some(t) = text(v)
                            {
                                out.push((rank, item(&group, words(name), t)));
                            }
                        }
                    } else if let Some(t) = text(value) {
                        out.push((s, item(words(section), words(key), t)));
                    }
                }
            }
            other => {
                if let Some(t) = text(other) {
                    out.push((s, item("Other", words(section), t)));
                }
            }
        }
    }
    out
}

// ----- F1 25 ----------------------------------------------------------------

/// The car setup packet (`sy_source_f1::packets::CarSetup::to_json`).
fn f1(setup: &Value) -> Vec<(u32, SetupItem)> {
    // (key, group, name, unit)
    const FIELDS: &[(&str, &str, &str, &str)] = &[
        ("front_wing", "Aerodynamics", "Front wing", ""),
        ("rear_wing", "Aerodynamics", "Rear wing", ""),
        (
            "diff_on_throttle",
            "Transmission",
            "Differential on throttle",
            "%",
        ),
        (
            "diff_off_throttle",
            "Transmission",
            "Differential off throttle",
            "%",
        ),
        ("engine_braking", "Transmission", "Engine braking", "%"),
        ("front_camber", "Suspension geometry", "Front camber", "°"),
        ("rear_camber", "Suspension geometry", "Rear camber", "°"),
        ("front_toe", "Suspension geometry", "Front toe", "°"),
        ("rear_toe", "Suspension geometry", "Rear toe", "°"),
        ("front_suspension", "Suspension", "Front suspension", ""),
        ("rear_suspension", "Suspension", "Rear suspension", ""),
        (
            "front_anti_roll_bar",
            "Suspension",
            "Front anti-roll bar",
            "",
        ),
        ("rear_anti_roll_bar", "Suspension", "Rear anti-roll bar", ""),
        ("front_ride_height", "Suspension", "Front ride height", ""),
        ("rear_ride_height", "Suspension", "Rear ride height", ""),
        ("brake_pressure", "Brakes", "Brake pressure", "%"),
        ("brake_bias", "Brakes", "Front brake bias", "%"),
        ("ballast", "Weight", "Ballast", ""),
        ("fuel_load", "Weight", "Fuel load", " kg"),
    ];
    const TIRES: &[(&str, &str)] = &[
        ("fl", "Front left pressure"),
        ("fr", "Front right pressure"),
        ("rl", "Rear left pressure"),
        ("rr", "Rear right pressure"),
    ];
    let mut out = Vec::new();
    let Some(map) = setup.as_object() else {
        return out;
    };
    for (i, (key, group, name, unit)) in FIELDS.iter().enumerate() {
        let value = map.get(*key).and_then(|v| match v.as_f64() {
            // the game sends floats: two decimals are what it shows
            Some(f) if v.is_f64() => Some(format!("{f:.2}")),
            _ => text(v),
        });
        if let Some(value) = value {
            let rank = if *group == "Weight" { 2000 } else { 0 } + i as u32;
            out.push((rank, item(*group, *name, format!("{value}{unit}"))));
        }
    }
    if let Some(Value::Object(p)) = map.get("tyre_pressure_psi") {
        for (i, (wheel, name)) in TIRES.iter().enumerate() {
            if let Some(psi) = p.get(*wheel).and_then(Value::as_f64) {
                out.push((
                    1000 + i as u32,
                    item("Tires", *name, format!("{psi:.1} psi")),
                ));
            }
        }
    }
    // anything a newer packet adds
    let known = |k: &str| k == "tyre_pressure_psi" || FIELDS.iter().any(|f| f.0 == k);
    for (key, v) in map {
        if !known(key)
            && let Some(t) = text(v)
        {
            out.push((3000, item("Other", snake_words(key), t)));
        }
    }
    out
}

/// `front_wing` → `Front wing`.
fn snake_words(key: &str) -> String {
    let lower = key.replace('_', " ").to_lowercase();
    let mut c = lower.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

// ----- Le Mans Ultimate -----------------------------------------------------

/// Sections of the LMU setup, in order.
const LMU_GROUPS: &[&str] = &[
    "Aerodynamics",
    "Tires",
    "Brakes",
    "Electronics",
    "Differential",
    "Suspension",
    "Dampers",
    "Gearing",
    "Fuel and pit stops",
    "Cooling",
    "Steering",
    "Other",
];

/// Section of a car setting (`VM_` key without the prefix).
fn lmu_car_group(key: &str) -> &'static str {
    let has = |s: &str| key.contains(s);
    if has("WING") {
        "Aerodynamics"
    } else if has("TIRE_COMPOUND") {
        "Tires"
    } else if has("TRACTION")
        || has("ANTILOCK")
        || has("MOTOR_MAP")
        || has("REGEN")
        || has("ENGINE_")
        || has("REV_LIMITER")
    {
        "Electronics"
    } else if has("BRAKE") {
        "Brakes"
    } else if has("DIFF") || has("TORQUE_SPLIT") {
        "Differential"
    } else if has("GEAR") || has("RATIO") {
        "Gearing"
    } else if has("FUEL") || has("PITSTOP") || has("VIRTUAL_ENERGY") {
        "Fuel and pit stops"
    } else if has("RADIATOR") {
        "Cooling"
    } else if has("STEER") {
        "Steering"
    } else if has("ANTISWAY")
        || has("TOE")
        || has("3RD")
        || has("CASTER")
        || has("TRACK")
        || has("WEIGHT")
        || has("CHASSIS")
        || has("FENDER")
    {
        "Suspension"
    } else {
        "Other"
    }
}

/// Section of a wheel setting (`WM_` key without the prefix and wheel).
fn lmu_wheel_group(key: &str) -> &'static str {
    match key {
        "PRESSURE" | "COMPOUND" => "Tires",
        "SLOWBUMP" | "SLOWREBOUND" | "FASTBUMP" | "FASTREBOUND" => "Dampers",
        "BRAKEDISC" | "BRAKEPAD" => "Brakes",
        _ => "Suspension",
    }
}

/// Readable words for the parts of a key.
fn lmu_words(key: &str) -> String {
    const WORDS: &[(&str, &str)] = &[
        ("ANTILOCKBRAKESYSTEMMAP", "ABS map"),
        ("ANTILOCK_BRAKES", "ABS"),
        ("TRACTIONCONTROLMAP", "TC map"),
        ("TRACTIONCONTROLPOWERCUTMAP", "TC power cut map"),
        ("TRACTIONCONTROLSLIPANGLEMAP", "TC slip angle map"),
        ("TRACTION_CONTROL", "Traction control"),
        ("ANTISWAY", "anti-roll bar"),
        ("TOEIN", "toe-in"),
        ("TOEOFFSET", "toe offset"),
        ("3RD", "third"),
        ("SLOWBUMP", "slow bump"),
        ("SLOWREBOUND", "slow rebound"),
        ("FASTBUMP", "fast bump"),
        ("FASTREBOUND", "fast rebound"),
        ("RIDEHEIGHT", "ride height"),
        ("TENDERSPRINGTRAVEL", "tender spring travel"),
        ("TENDERSPRING", "tender spring"),
        ("BRAKEDISC", "brake disc"),
        ("BRAKEPAD", "brake pad"),
        ("BRAKEMAP", "brake map"),
        ("SRUBBER", "spring rubber"),
        ("AUTODOWNSHIFT", "auto downshift"),
        ("AUTOUPSHIFT", "auto upshift"),
        ("UPSHIFT_RPM", "upshift rpm"),
        ("PACKERS", "packers"),
        ("NUM_PITSTOPS", "pit stops"),
        ("PITSTOP", "pit stop"),
    ];
    let mut s = key.to_owned();
    for (from, to) in WORDS {
        s = s.replace(from, &format!("\u{1}{to}\u{1}"));
    }
    let lower: String = s
        .split('\u{1}')
        .enumerate()
        .map(|(i, part)| {
            if i % 2 == 1 {
                part.to_owned() // already readable
            } else {
                part.replace('_', " ").to_lowercase()
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut c = lower.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// `89.0L (25.5 laps)` → `89.0L`: the game's estimate of how far fuel and
/// energy last changes during a session without the setup changing.
fn without_lap_estimate(value: &str) -> String {
    match value.rsplit_once(" (") {
        Some((v, rest))
            if rest
                .strip_suffix(" laps)")
                .is_some_and(|n| n.parse::<f64>().is_ok()) =>
        {
            v.trim_end().to_owned()
        }
        _ => value.to_owned(),
    }
}

/// `{"VM_BRAKE_BALANCE": "51.2:48.8", "WM_CAMBER-W_FL": "-2.20 deg", …}`
/// (`sy_source_lmu::decoder::compact_setup`).
fn lmu(setup: &Value) -> Vec<(u32, SetupItem)> {
    let mut out = Vec::new();
    let Some(map) = setup.as_object() else {
        return out;
    };
    let rank_of = |group: &str| {
        LMU_GROUPS
            .iter()
            .position(|g| *g == group)
            .unwrap_or(LMU_GROUPS.len()) as u32
            * 1000
    };
    for (key, v) in map {
        // garage UI state, not a setting
        if key == "symmetric" || key == "gearGraph" {
            continue;
        }
        let Some(value) = text(v).map(|t| without_lap_estimate(&t)) else {
            continue;
        };
        // keys come sorted, so a wheel setting's FL, FR, RL, RR stay together
        let (group, name) = if let Some(car) = key.strip_prefix("VM_") {
            (lmu_car_group(car), lmu_words(car))
        } else if let Some((setting, wheel)) =
            key.strip_prefix("WM_").and_then(|k| k.split_once("-W_"))
        {
            (
                lmu_wheel_group(setting),
                format!("{} {wheel}", lmu_words(setting)),
            )
        } else {
            ("Other", lmu_words(key))
        };
        out.push((rank_of(group), item(group, name, value)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rows(items: &[SetupItem]) -> Vec<String> {
        items
            .iter()
            .map(|i| format!("{} | {} | {}", i.group, i.name, i.value))
            .collect()
    }

    #[test]
    fn camel_case_words() {
        assert_eq!(words("LeftFront"), "Left front");
        assert_eq!(words("ColdPressure"), "Cold pressure");
        assert_eq!(words("ArbPreloadAdj"), "ARB preload adj");
        assert_eq!(words("ARBSetting"), "ARB setting");
        assert_eq!(words("InCarDials"), "In car dials");
        assert_eq!(words("Rear"), "Rear");
    }

    #[test]
    fn iracing_setup_without_measured_values() {
        let setup = json!({
            "UpdateCount": "2",
            "Suspension": {
                "Front": {"ToeIn": "-1.3 mm", "AntiRollBar": "Med"},
                "LeftFront": {
                    "ColdPressure": "193 kPa", "LastHotPressure": "193 kPa",
                    "LastTempsOMI": "42C, 42C, 42C", "TreadRemaining": "100%, 100%, 100%",
                    "Camber": "-3.5 deg"
                },
                "RightFront": {"Camber": "-3.4 deg"},
                "Rear": {"FuelLevel": "23.8 L"}
            }
        });
        let items = setup_items(GameId::Iracing, &setup);
        assert_eq!(
            rows(&items),
            [
                "Suspension · Front | Anti roll bar | Med",
                "Suspension · Front | Toe in | -1.3 mm",
                "Suspension · Left front | Camber | -3.5 deg",
                "Suspension · Left front | Cold pressure | 193 kPa",
                "Suspension · Right front | Camber | -3.4 deg",
                "Suspension · Rear | Fuel level | 23.8 L",
            ]
        );
    }

    #[test]
    fn f1_setup_with_units() {
        let setup = json!({
            "front_wing": 30, "rear_wing": 28, "brake_bias": 56,
            "front_camber": -3.099_999_904_632_568_4, "fuel_load": 10.0,
            "tyre_pressure_psi": {"fl": 22.5, "fr": 22.5, "rl": 20.0, "rr": 20.0},
            "new_thing": 4
        });
        let items = setup_items(GameId::F1_25, &setup);
        assert_eq!(
            rows(&items),
            [
                "Aerodynamics | Front wing | 30",
                "Aerodynamics | Rear wing | 28",
                "Suspension geometry | Front camber | -3.10°",
                "Brakes | Front brake bias | 56%",
                "Tires | Front left pressure | 22.5 psi",
                "Tires | Front right pressure | 22.5 psi",
                "Tires | Rear left pressure | 20.0 psi",
                "Tires | Rear right pressure | 20.0 psi",
                "Weight | Fuel load | 10.00 kg",
                "Other | New thing | 4",
            ]
        );
    }

    #[test]
    fn lmu_setup_grouped() {
        let setup = json!({
            "VM_BRAKE_BALANCE": "51.2:48.8",
            "VM_FRONT_ANTISWAY": "P4",
            "VM_REAR_WING": "1.4 deg",
            "VM_TRACTIONCONTROLMAP": "7",
            "VM_FUEL_CAPACITY": "89.0L (25.5 laps)",
            "VM_STEER_LOCK": "516 deg(19.7 )",
            "WM_CAMBER-W_FL": "-2.20 deg",
            "WM_CAMBER-W_FR": "-2.20 deg",
            "WM_PRESSURE-W_FL": "136 kPa",
            "WM_SLOWBUMP-W_RL": "5",
            "symmetric": true
        });
        let items = setup_items(GameId::Lmu, &setup);
        assert_eq!(
            rows(&items),
            [
                "Aerodynamics | Rear wing | 1.4 deg",
                "Tires | Pressure FL | 136 kPa",
                "Brakes | Brake balance | 51.2:48.8",
                "Electronics | TC map | 7",
                "Suspension | Front anti-roll bar | P4",
                "Suspension | Camber FL | -2.20 deg",
                "Suspension | Camber FR | -2.20 deg",
                "Dampers | Slow bump RL | 5",
                "Fuel and pit stops | Fuel capacity | 89.0L",
                "Steering | Steer lock | 516 deg(19.7 )",
            ]
        );
    }

    #[test]
    fn fixed_setup_from_the_session_info() {
        let info = json!({"fixed_setup": true, "player": {"user_name": "Me"}});
        assert!(fixed_setup(Some(&info)));
        assert!(!fixed_setup(Some(&json!({"fixed_setup": false}))));
        assert!(!fixed_setup(Some(&json!({"track": {}}))));
        assert!(!fixed_setup(None));
        assert_eq!(
            shared_session_info(Some(&info)),
            Some(json!({"fixed_setup": true}))
        );
        assert_eq!(shared_session_info(Some(&json!({"track": {}}))), None);
    }

    #[test]
    fn session_setup_only_for_laps_without_one() {
        let lap = json!({"front_wing": 30});
        let session = json!({"front_wing": 25});
        let s = lap_setup(GameId::F1_25, Some(&lap), Some(&session)).unwrap();
        assert_eq!(
            (s.source, s.items[0].value.as_str()),
            (SetupSource::Lap, "30")
        );
        let s = lap_setup(GameId::F1_25, None, Some(&session)).unwrap();
        assert_eq!(
            (s.source, s.items[0].value.as_str()),
            (SetupSource::Session, "25")
        );
        assert!(lap_setup(GameId::F1_25, None, None).is_none());
        // nothing readable
        assert!(lap_setup(GameId::F1_25, Some(&json!([])), None).is_none());
    }
}
