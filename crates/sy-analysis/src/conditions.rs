// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Weather, fuel and tyres of one lap, from its telemetry (the lap table's
//! extra columns). Computed when a session is shown, so laps recorded before
//! a game adapter recorded a value simply have none.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};

use sy_model::api::{LapConditions, TyreCompound, TyreNames, WheelFlags, Wheels};
use sy_model::channel::names;
use sy_model::{BlobHash, Channel, ChannelData, GameId, LapTelemetry, Unit};

/// Results kept by telemetry file (a lap's file never changes) and the
/// session's compound names.
type Cache = Mutex<HashMap<(GameId, BlobHash, String), LapConditions>>;

/// Names of a session's tyre compound indices. iRacing and LMU record an
/// index; the names are in the session info (F1 has fixed codes).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompoundNames {
    front: BTreeMap<i32, String>,
    rear: BTreeMap<i32, String>,
}

impl CompoundNames {
    /// From the session info (`player.tyre_compounds` for iRacing,
    /// `tyre_compounds_front` / `_rear` for LMU).
    pub fn from_session_info(game: GameId, info: Option<&serde_json::Value>) -> Self {
        let Some(info) = info else {
            return Self::default();
        };
        let table = |v: &serde_json::Value| -> BTreeMap<i32, String> {
            match v {
                serde_json::Value::Array(a) => a
                    .iter()
                    .enumerate()
                    .filter_map(|(i, n)| Some((i as i32, n.as_str()?.to_owned())))
                    .filter(|(_, n)| !n.is_empty())
                    .collect(),
                serde_json::Value::Object(o) => o
                    .iter()
                    .filter_map(|(i, n)| Some((i.parse().ok()?, n.as_str()?.to_owned())))
                    .filter(|(_, n)| !n.is_empty())
                    .collect(),
                _ => BTreeMap::new(),
            }
        };
        match game {
            GameId::Iracing => {
                let names: BTreeMap<i32, String> = table(&info["player"]["tyre_compounds"])
                    .into_iter()
                    .map(|(i, n)| (i, readable(&n)))
                    .collect();
                Self {
                    front: names.clone(),
                    rear: names,
                }
            }
            GameId::Lmu => Self {
                front: table(&info["tyre_compounds_front"]),
                rear: table(&info["tyre_compounds_rear"]),
            },
            GameId::F1_25 => Self::default(),
        }
    }

    /// Part of the cache key.
    fn key(&self) -> String {
        format!("{:?}{:?}", self.front, self.rear)
    }
}

/// Entries before the cache starts over (about 100 bytes each).
const CACHE_MAX: usize = 50_000;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Cache::default)
}

/// Conditions of many laps (the lap table), one per telemetry file hash.
/// Results are cached (a lap's file never changes); the others are read
/// with `load`, one after the other, and decoded in parallel.
pub fn laps_conditions<E: std::fmt::Display>(
    game: GameId,
    names: &CompoundNames,
    hashes: &[&BlobHash],
    mut load: impl FnMut(&BlobHash) -> Result<Vec<u8>, E>,
) -> Vec<Result<LapConditions, String>> {
    let names_key = names.key();
    let mut out: Vec<Option<Result<LapConditions, String>>> = {
        let cache = cache().lock().unwrap_or_else(|e| e.into_inner());
        hashes
            .iter()
            .map(|h| {
                cache
                    .get(&(game, (*h).clone(), names_key.clone()))
                    .cloned()
                    .map(Ok)
            })
            .collect()
    };
    let mut todo: Vec<(usize, Vec<u8>)> = Vec::new();
    for (i, slot) in out.iter_mut().enumerate() {
        if slot.is_none() {
            match load(hashes[i]) {
                Ok(bytes) => todo.push((i, bytes)),
                Err(e) => *slot = Some(Err(e.to_string())),
            }
        }
    }
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let chunk = todo.len().div_ceil(threads).max(1);
    let decoded: Vec<(usize, Result<LapConditions, String>)> = std::thread::scope(|s| {
        let workers: Vec<_> = todo
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    part.iter()
                        .map(|(i, bytes)| {
                            let t = LapTelemetry::from_parquet_channels(bytes.clone(), |name| {
                                is_conditions_channel(game, name)
                            });
                            (
                                *i,
                                t.map(|t| lap_conditions_named(game, &t, names))
                                    .map_err(|e| e.to_string()),
                            )
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().unwrap_or_default())
            .collect()
    });
    {
        let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() + decoded.len() > CACHE_MAX {
            cache.clear();
        }
        for (i, result) in &decoded {
            if let Ok(c) = result {
                cache.insert((game, hashes[*i].clone(), names_key.clone()), c.clone());
            }
        }
    }
    for (i, result) in decoded {
        out[i] = Some(result);
    }
    out.into_iter()
        .map(|r| r.unwrap_or_else(|| Err("telemetry not decoded".into())))
        .collect()
}

/// Per-wheel channel suffixes in [`Wheels`] order.
const WHEELS: [&str; 4] = ["_fl", "_fr", "_rl", "_rr"];

/// Tyre surface temperature per wheel (prefix of `<prefix>_fl` …).
fn tyre_temp_prefix(game: GameId) -> &'static str {
    match game {
        GameId::Iracing => "iracing.tyre_temp",
        GameId::F1_25 => "f1.tyre_surface_temp",
        GameId::Lmu => "lmu.tyre_temp",
    }
}

/// Tyre wear per wheel, 0 = new … 1 = worn out. iRacing only updates wear in
/// the pits, so it isn't a per-lap value there.
fn tyre_wear_prefix(game: GameId) -> Option<&'static str> {
    match game {
        GameId::Iracing => None,
        GameId::F1_25 => Some("f1.tyre_wear"),
        GameId::Lmu => Some("lmu.tyre_wear"),
    }
}

/// Flat tyre / detached wheel flags per wheel (prefixes). LMU reports a
/// flat tyre as fully worn, so wear is taken from before the puncture.
fn tyre_flat_prefixes(game: GameId) -> &'static [&'static str] {
    match game {
        GameId::Lmu => &["lmu.tyre_flat", "lmu.wheel_detached"],
        GameId::Iracing | GameId::F1_25 => &[],
    }
}

/// Game-specific channels behind the weather description.
fn weather_channels(game: GameId) -> &'static [&'static str] {
    match game {
        GameId::Iracing => &[
            "iracing.skies",
            "iracing.precipitation",
            "iracing.track_wetness",
        ],
        GameId::F1_25 => &["f1.weather"],
        GameId::Lmu => &["lmu.rain", "lmu.track_wetness"],
    }
}

/// Whether [`lap_conditions`] reads this channel: load only these.
pub fn is_conditions_channel(game: GameId, name: &str) -> bool {
    let wheel_of = |prefix: &str| {
        name.strip_prefix(prefix)
            .is_some_and(|rest| WHEELS.contains(&rest))
    };
    [
        names::FUEL,
        names::AIR_TEMP,
        names::TRACK_TEMP,
        names::HUMIDITY,
    ]
    .contains(&name)
        || weather_channels(game).contains(&name)
        || wheel_of(tyre_temp_prefix(game))
        || tyre_wear_prefix(game).is_some_and(wheel_of)
        || tyre_flat_prefixes(game).iter().any(|p| wheel_of(p))
        || compound_channels(game).contains(&name)
        || (game == GameId::Lmu
            && (wheel_of("lmu.tyre_compound") || wheel_of("lmu.tyre_compound_type")))
}

/// Channels behind the tyre compound.
fn compound_channels(game: GameId) -> &'static [&'static str] {
    match game {
        GameId::Iracing => &["iracing.tyre_compound"],
        GameId::F1_25 => &[
            "f1.tyre_compound",
            "f1.tyre_visual_compound",
            "f1.tyre_age_laps",
        ],
        // axle compound: laps recorded before the per-wheel one
        GameId::Lmu => &["lmu.tyre_compound_front", "lmu.tyre_compound_rear"],
    }
}

/// iRacing's compound names as words: `AllPurpose` → `All purpose`.
fn readable(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if i > 0 && c.is_uppercase() {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// F1 visual compound (what the game shows), see the UDP spec.
fn f1_visual(code: i32) -> Option<&'static str> {
    Some(match code {
        16 | 20 => "Soft",
        17 | 21 => "Medium",
        18 | 22 => "Hard",
        19 => "Super soft",
        7 => "Intermediate",
        8 | 10 | 15 => "Wet",
        9 => "Dry",
        _ => return None,
    })
}

/// F1 actual compound: the C number (modern F1 only).
fn f1_actual(code: i32) -> Option<&'static str> {
    Some(match code {
        21 => "C0",
        20 => "C1",
        19 => "C2",
        18 => "C3",
        17 => "C4",
        16 => "C5",
        22 => "C6",
        _ => return None,
    })
}

fn first_last(t: &LapTelemetry, name: &str) -> Option<(i32, i32)> {
    let c = t.channel(name)?;
    let first = finite(c).next()?;
    let last = finite(c).last()?;
    Some((first.round() as i32, last.round() as i32))
}

/// LMU's compound type (`mCompoundType`), the garage's per-wheel setting
/// (`WM_COMPOUND-W_*`): 0 soft, 1 medium, 2 hard, 3 wet. Only used if the
/// game named no compound with the wheel's index.
fn lmu_type_name(t: i32) -> Option<&'static str> {
    Some(match t {
        0 => "Soft",
        1 => "Medium",
        2 => "Hard",
        3 => "Wet",
        _ => return None,
    })
}

/// The tyres a lap was driven on: the compound at its end (and at its start
/// if they were changed in it).
fn tyre_compound(game: GameId, t: &LapTelemetry, names: &CompoundNames) -> Option<TyreCompound> {
    let all = |n: &str| [n.to_owned(), n.to_owned(), n.to_owned(), n.to_owned()];
    let (end, start, detail, age_laps) = match game {
        GameId::F1_25 => {
            let (s, e) = first_last(t, "f1.tyre_visual_compound")?;
            let changed = s != e;
            // the set's age at the lap start; after a change, the new set's
            let age = first_last(t, "f1.tyre_age_laps").map(|(a, b)| if changed { b } else { a });
            (
                all(f1_visual(e)?),
                f1_visual(s).map(all),
                first_last(t, "f1.tyre_compound")
                    .and_then(|(_, a)| f1_actual(a))
                    .map(str::to_owned),
                age.map(|a| a.max(0) as u32),
            )
        }
        GameId::Iracing => {
            let (s, e) = first_last(t, "iracing.tyre_compound")?;
            (
                all(names.front.get(&e)?),
                names.front.get(&s).map(|n| all(n)),
                None,
                None,
            )
        }
        GameId::Lmu => {
            // per wheel: the name the game gave the index (front tables for
            // the front wheels), else the compound type
            let wheel = |k: usize, at_end: bool| -> Option<String> {
                let pick = |(a, b): (i32, i32)| if at_end { b } else { a };
                let (own, other) = if k < 2 {
                    (&names.front, &names.rear)
                } else {
                    (&names.rear, &names.front)
                };
                let named = |i: i32| own.get(&i).or_else(|| other.get(&i)).cloned();
                if let Some(index) = first_last(t, &format!("lmu.tyre_compound{}", WHEELS[k])) {
                    let kind = first_last(t, &format!("lmu.tyre_compound_type{}", WHEELS[k]));
                    return named(pick(index))
                        .or_else(|| kind.and_then(|k| lmu_type_name(pick(k))).map(str::to_owned));
                }
                // laps recorded with the axle's compound only
                let axle = if k < 2 {
                    "lmu.tyre_compound_front"
                } else {
                    "lmu.tyre_compound_rear"
                };
                named(pick(first_last(t, axle)?))
            };
            let four = |at_end: bool| -> Option<[String; 4]> {
                Some([
                    wheel(0, at_end)?,
                    wheel(1, at_end)?,
                    wheel(2, at_end)?,
                    wheel(3, at_end)?,
                ])
            };
            (four(true)?, four(false), None, None)
        }
    };
    let before = start
        .filter(|s| *s != end)
        .map(|[fl, fr, rl, rr]| TyreNames { fl, fr, rl, rr });
    let [fl, fr, rl, rr] = end;
    Some(TyreCompound {
        fl,
        fr,
        rl,
        rr,
        detail,
        before,
        age_laps,
    })
}

/// Weather, fuel and tyres of one lap (compound names only where the game
/// has fixed codes: F1).
pub fn lap_conditions(game: GameId, t: &LapTelemetry) -> LapConditions {
    lap_conditions_named(game, t, &CompoundNames::default())
}

/// Weather, fuel and tyres of one lap, with the session's compound names.
pub fn lap_conditions_named(
    game: GameId,
    t: &LapTelemetry,
    names: &CompoundNames,
) -> LapConditions {
    let fuel = t.channel(names::FUEL);
    let fuel_start = fuel.and_then(|c| finite(c).next());
    let fuel_end = fuel.and_then(|c| finite(c).last());
    LapConditions {
        air_temp_c: mean(t, names::AIR_TEMP),
        track_temp_c: mean(t, names::TRACK_TEMP),
        humidity: mean(t, names::HUMIDITY),
        weather: weather(game, t),
        fuel_start,
        // refuelled during the lap (pit stop): no consumption
        fuel_used: fuel_start
            .zip(fuel_end)
            .map(|(a, b)| a - b)
            .filter(|&used| used >= 0.0),
        fuel_unit: fuel.and_then(|c| match c.unit {
            Unit::Liter => Some("l".into()),
            Unit::Kilogram => Some("kg".into()),
            _ => None,
        }),
        // below -100 °C isn't a reading (LMU laps recorded before 0 K was
        // dropped: the game zeroes the wheels when going back to the garage)
        tyre_temp_c: wheels(t, tyre_temp_prefix(game), |c| {
            average(finite(c).filter(|&x| x > -100.0))
        }),
        tyre_wear: tyre_wear_prefix(game).and_then(|p| wear(game, t, p)),
        tyre_flat: flats(game, t),
        tyre_compound: tyre_compound(game, t, names),
    }
}

/// Whether the wheel's tyre is flat (or the wheel came off), per sample.
fn flat_samples(game: GameId, t: &LapTelemetry, suffix: &str) -> Vec<bool> {
    let mut flat = vec![false; t.len()];
    for prefix in tyre_flat_prefixes(game) {
        if let Some(c) = t.channel(&format!("{prefix}{suffix}")) {
            for (f, x) in flat.iter_mut().zip(values(c)) {
                *f |= x != 0.0;
            }
        }
    }
    flat
}

/// Wear at the end of the lap, or before the tyre went flat.
fn wear(game: GameId, t: &LapTelemetry, prefix: &str) -> Option<Wheels> {
    let at = |suffix: &str| {
        let c = t.channel(&format!("{prefix}{suffix}"))?;
        let flat = flat_samples(game, t, suffix);
        let intact = values(c)
            .zip(&flat)
            .filter(|&(x, &f)| x.is_finite() && !f)
            .map(|(x, _)| x)
            .last();
        intact
            .or_else(|| finite(c).last())
            .map(|x| x.clamp(0.0, 1.0))
    };
    Some(Wheels {
        fl: at(WHEELS[0])?,
        fr: at(WHEELS[1])?,
        rl: at(WHEELS[2])?,
        rr: at(WHEELS[3])?,
    })
}

fn flats(game: GameId, t: &LapTelemetry) -> Option<WheelFlags> {
    let any = |suffix: &str| flat_samples(game, t, suffix).contains(&true);
    let f = WheelFlags {
        fl: any(WHEELS[0]),
        fr: any(WHEELS[1]),
        rl: any(WHEELS[2]),
        rr: any(WHEELS[3]),
    };
    (f != WheelFlags::default()).then_some(f)
}

/// All samples of a channel, NaN kept (aligned with other channels).
fn values(c: &Channel) -> Box<dyn Iterator<Item = f32> + '_> {
    match &c.data {
        ChannelData::F32(v) => Box::new(v.iter().copied()),
        ChannelData::F64(v) => Box::new(v.iter().map(|&x| x as f32)),
        ChannelData::I32(v) => Box::new(v.iter().map(|&x| x as f32)),
        ChannelData::Bool(v) => Box::new(v.iter().map(|&x| f32::from(u8::from(x)))),
    }
}

/// The finite samples of a channel.
fn finite(c: &Channel) -> Box<dyn Iterator<Item = f32> + '_> {
    match &c.data {
        ChannelData::F32(v) => Box::new(v.iter().copied().filter(|x| x.is_finite())),
        ChannelData::F64(v) => Box::new(v.iter().filter(|x| x.is_finite()).map(|&x| x as f32)),
        ChannelData::I32(v) => Box::new(v.iter().map(|&x| x as f32)),
        ChannelData::Bool(v) => Box::new(v.iter().map(|&x| f32::from(u8::from(x)))),
    }
}

fn average(values: impl Iterator<Item = f32>) -> Option<f32> {
    let (sum, n) = values.fold((0.0_f64, 0_u32), |(s, n), x| (s + f64::from(x), n + 1));
    (n > 0).then(|| (sum / f64::from(n)) as f32)
}

fn mean(t: &LapTelemetry, name: &str) -> Option<f32> {
    t.channel(name).and_then(|c| average(finite(c)))
}

/// The most frequent value of an enumeration channel (rounded).
fn mode(t: &LapTelemetry, name: &str) -> Option<i32> {
    let mut counts = std::collections::BTreeMap::<i32, usize>::new();
    for x in finite(t.channel(name)?) {
        *counts.entry(x.round() as i32).or_default() += 1;
    }
    counts.into_iter().max_by_key(|&(_, n)| n).map(|(v, _)| v)
}

/// One value per wheel; None unless all four wheels have one.
fn wheels(
    t: &LapTelemetry,
    prefix: &str,
    value: impl Fn(&Channel) -> Option<f32>,
) -> Option<Wheels> {
    let get = |suffix: &str| t.channel(&format!("{prefix}{suffix}")).and_then(&value);
    Some(Wheels {
        fl: get(WHEELS[0])?,
        fr: get(WHEELS[1])?,
        rl: get(WHEELS[2])?,
        rr: get(WHEELS[3])?,
    })
}

fn percent(ratio: f32) -> String {
    format!("{:.0} %", ratio * 100.0)
}

fn weather(game: GameId, t: &LapTelemetry) -> Option<String> {
    match game {
        GameId::F1_25 => Some(
            match mode(t, "f1.weather")? {
                0 => "Clear",
                1 => "Light cloud",
                2 => "Overcast",
                3 => "Light rain",
                4 => "Heavy rain",
                5 => "Storm",
                _ => return None,
            }
            .into(),
        ),
        GameId::Lmu => {
            let rain = mean(t, "lmu.rain")?;
            let mut text = if rain >= 0.05 {
                format!("Rain {}", percent(rain))
            } else {
                "Dry".into()
            };
            if let Some(wet) = mean(t, "lmu.track_wetness").filter(|&w| w >= 0.05) {
                text += &format!(", track wet {}", percent(wet));
            }
            Some(text)
        }
        GameId::Iracing => {
            let mut text: String = match mode(t, "iracing.skies")? {
                0 => "Clear",
                1 => "Partly cloudy",
                2 => "Mostly cloudy",
                3 => "Overcast",
                _ => return None,
            }
            .into();
            if let Some(rain) = mean(t, "iracing.precipitation").filter(|&r| r >= 0.05) {
                text += &format!(", rain {}", percent(rain));
            }
            // irsdk_TrackWetness; 0 = unknown, 1 = dry
            let wetness = match mode(t, "iracing.track_wetness") {
                Some(2) => Some("mostly dry"),
                Some(3) => Some("very lightly wet"),
                Some(4) => Some("lightly wet"),
                Some(5) => Some("moderately wet"),
                Some(6) => Some("very wet"),
                Some(7) => Some("extremely wet"),
                _ => None,
            };
            if let Some(w) = wetness {
                text += &format!(", track {w}");
            }
            Some(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tel(channels: Vec<(&str, Unit, Vec<f32>)>) -> LapTelemetry {
        let mut t = LapTelemetry::new(60.0);
        for (name, unit, v) in channels {
            t.add_channel(Channel::new(name, unit, ChannelData::F32(v)))
                .unwrap();
        }
        t
    }

    fn wheels_of(prefix: &str, unit: Unit, values: [Vec<f32>; 4]) -> Vec<(String, Unit, Vec<f32>)> {
        WHEELS
            .iter()
            .zip(values)
            .map(|(w, v)| (format!("{prefix}{w}"), unit, v))
            .collect()
    }

    #[test]
    fn fuel_temperatures_and_tyres() {
        let mut ch: Vec<(String, Unit, Vec<f32>)> = vec![
            (
                names::FUEL.into(),
                Unit::Liter,
                vec![f32::NAN, 50.0, 49.0, 48.0],
            ),
            (
                names::AIR_TEMP.into(),
                Unit::Celsius,
                vec![20.0, 22.0, f32::NAN, 21.0],
            ),
            (names::TRACK_TEMP.into(), Unit::Celsius, vec![30.0; 4]),
            ("lmu.rain".into(), Unit::Ratio, vec![0.0; 4]),
            ("lmu.track_wetness".into(), Unit::Ratio, vec![0.2; 4]),
        ];
        ch.extend(wheels_of(
            "lmu.tyre_temp",
            Unit::Celsius,
            [
                vec![80.0, 90.0, 100.0, 90.0],
                vec![90.0; 4],
                vec![85.0; 4],
                vec![86.0; 4],
            ],
        ));
        ch.extend(wheels_of(
            "lmu.tyre_wear",
            Unit::Ratio,
            [
                vec![0.1, 0.12, 0.13, 0.14],
                vec![0.1; 4],
                vec![0.05; 4],
                vec![0.06; 4],
            ],
        ));
        let t = tel(ch
            .iter()
            .map(|(n, u, v)| (n.as_str(), *u, v.clone()))
            .collect());
        let c = lap_conditions(GameId::Lmu, &t);
        assert_eq!(c.fuel_start, Some(50.0));
        assert_eq!(c.fuel_used, Some(2.0));
        assert_eq!(c.fuel_unit.as_deref(), Some("l"));
        assert_eq!(c.air_temp_c, Some(21.0));
        assert_eq!(c.track_temp_c, Some(30.0));
        assert_eq!(c.humidity, None);
        assert_eq!(c.weather.as_deref(), Some("Dry, track wet 20 %"));
        let temp = c.tyre_temp_c.unwrap();
        assert_eq!((temp.fl, temp.rr), (90.0, 86.0));
        let wear = c.tyre_wear.unwrap();
        assert_eq!((wear.fl, wear.rl), (0.14, 0.05));
    }

    #[test]
    fn refuelling_has_no_consumption() {
        let t = tel(vec![(names::FUEL, Unit::Kilogram, vec![5.0, 4.0, 60.0])]);
        let c = lap_conditions(GameId::F1_25, &t);
        assert_eq!((c.fuel_start, c.fuel_used), (Some(5.0), None));
        assert_eq!(c.fuel_unit.as_deref(), Some("kg"));
    }

    #[test]
    fn weather_per_game() {
        let f1 = tel(vec![("f1.weather", Unit::None, vec![1.0, 3.0, 3.0])]);
        assert_eq!(
            lap_conditions(GameId::F1_25, &f1).weather.as_deref(),
            Some("Light rain")
        );
        let ir = tel(vec![
            ("iracing.skies", Unit::None, vec![1.0; 3]),
            ("iracing.precipitation", Unit::Ratio, vec![0.3; 3]),
            ("iracing.track_wetness", Unit::None, vec![4.0; 3]),
        ]);
        assert_eq!(
            lap_conditions(GameId::Iracing, &ir).weather.as_deref(),
            Some("Partly cloudy, rain 30 %, track lightly wet")
        );
        assert!(lap_conditions(GameId::Lmu, &LapTelemetry::new(60.0)).is_empty());
    }

    #[test]
    fn wear_before_a_puncture() {
        // LMU: the punctured tyre reads as worn out from the puncture on
        let mut t = tel(Vec::new());
        for (w, wear) in WHEELS.iter().zip([0.05, 0.05, 0.04, 0.04]) {
            let mut v = vec![wear; 4];
            let mut flat = vec![false; 4];
            if *w == "_fr" {
                v[2..].fill(1.0);
                flat[2..].fill(true);
            }
            t.add_channel(Channel::new(
                format!("lmu.tyre_wear{w}"),
                Unit::Ratio,
                ChannelData::F32(v),
            ))
            .unwrap();
            t.add_channel(Channel::new(
                format!("lmu.tyre_flat{w}"),
                Unit::None,
                ChannelData::Bool(flat),
            ))
            .unwrap();
        }
        let c = lap_conditions(GameId::Lmu, &t);
        assert_eq!(c.tyre_wear.unwrap().fr, 0.05);
        assert_eq!(
            c.tyre_flat,
            Some(WheelFlags {
                fr: true,
                ..WheelFlags::default()
            })
        );
        assert!(is_conditions_channel(GameId::Lmu, "lmu.wheel_detached_rl"));
    }

    #[test]
    fn many_laps_are_cached_by_file() {
        let (bytes, hash) = tel(vec![(names::FUEL, Unit::Liter, vec![12.0, 11.5])])
            .to_blob()
            .unwrap();
        let missing = BlobHash::of(b"missing");
        let mut loads = 0;
        let mut load = |h: &BlobHash| {
            loads += 1;
            if *h == hash {
                Ok(bytes.clone())
            } else {
                Err("no such file")
            }
        };
        let first = laps_conditions(
            GameId::Lmu,
            &CompoundNames::default(),
            &[&hash, &missing],
            &mut load,
        );
        assert_eq!(first[0].as_ref().unwrap().fuel_used, Some(0.5));
        assert_eq!(first[1].as_ref().unwrap_err(), "no such file");
        let again = laps_conditions(GameId::Lmu, &CompoundNames::default(), &[&hash], &mut load);
        assert_eq!(again[0].as_ref().unwrap().fuel_start, Some(12.0));
        assert_eq!(loads, 2, "the second time comes from the cache");
    }

    #[test]
    fn impossible_tyre_temperatures_are_ignored() {
        let mut ch = wheels_of(
            "lmu.tyre_temp",
            Unit::Celsius,
            [
                vec![80.0, 82.0, -273.15],
                vec![70.0; 3],
                vec![60.0; 3],
                vec![50.0; 3],
            ],
        );
        ch.push((names::FUEL.into(), Unit::Liter, vec![1.0; 3]));
        let t = tel(ch
            .iter()
            .map(|(n, u, v)| (n.as_str(), *u, v.clone()))
            .collect());
        assert_eq!(
            lap_conditions(GameId::Lmu, &t).tyre_temp_c.unwrap().fl,
            81.0
        );
    }

    #[test]
    fn f1_compound_and_a_change_in_the_lap() {
        let t = tel(vec![
            (
                "f1.tyre_visual_compound",
                Unit::None,
                vec![17.0, 17.0, 18.0],
            ),
            ("f1.tyre_compound", Unit::None, vec![17.0, 17.0, 18.0]),
            ("f1.tyre_age_laps", Unit::None, vec![12.0, 12.0, 0.0]),
        ]);
        let c = lap_conditions(GameId::F1_25, &t).tyre_compound.unwrap();
        assert_eq!([c.fl, c.fr, c.rl, c.rr], ["Hard", "Hard", "Hard", "Hard"]);
        assert_eq!(c.detail.as_deref(), Some("C3"));
        assert_eq!(c.before.map(|b| b.rr).as_deref(), Some("Medium"));
        assert_eq!(c.age_laps, Some(0), "the new set's age");
    }

    #[test]
    fn compound_names_from_the_session_info() {
        let ir =
            serde_json::json!({ "player": { "tyre_compounds": ["Hard", "Wet", "AllPurpose"] } });
        let names = CompoundNames::from_session_info(GameId::Iracing, Some(&ir));
        let t = tel(vec![("iracing.tyre_compound", Unit::None, vec![2.0; 3])]);
        let c = lap_conditions_named(GameId::Iracing, &t, &names)
            .tyre_compound
            .unwrap();
        assert_eq!(c.rr, "All purpose");
        // without the names (e.g. a copy of a shared session): none
        assert_eq!(lap_conditions(GameId::Iracing, &t).tyre_compound, None);

        let lmu = serde_json::json!({
            "tyre_compounds_front": { "0": "Soft", "1": "Medium" },
            "tyre_compounds_rear": { "1": "Medium" }
        });
        let names = CompoundNames::from_session_info(GameId::Lmu, Some(&lmu));
        let t = tel(vec![
            ("lmu.tyre_compound_front", Unit::None, vec![0.0; 3]),
            ("lmu.tyre_compound_rear", Unit::None, vec![1.0; 3]),
        ]);
        let c = lap_conditions_named(GameId::Lmu, &t, &names)
            .tyre_compound
            .unwrap();
        assert_eq!(
            [c.fl, c.rr],
            ["Soft", "Medium"],
            "front / rear (older laps)"
        );
        assert!(is_conditions_channel(GameId::Lmu, "lmu.tyre_compound_rear"));

        // per wheel: four different, one never named (its type then)
        let mut t = LapTelemetry::new(60.0);
        for (w, (index, kind)) in WHEELS.iter().zip([(0, 0), (1, 1), (0, 0), (5, 2)]) {
            for (name, v) in [
                ("lmu.tyre_compound", index),
                ("lmu.tyre_compound_type", kind),
            ] {
                t.add_channel(Channel::new(
                    format!("{name}{w}"),
                    Unit::None,
                    ChannelData::I32(vec![v; 3]),
                ))
                .unwrap();
            }
        }
        let c = lap_conditions_named(GameId::Lmu, &t, &names)
            .tyre_compound
            .unwrap();
        assert_eq!([c.fl, c.fr, c.rl, c.rr], ["Soft", "Medium", "Soft", "Hard"]);
        assert_eq!(c.before, None);

        // changed in the lap from a mixed set to four mediums
        let mut t = LapTelemetry::new(60.0);
        for (w, start) in WHEELS.iter().zip([0, 1, 0, 5]) {
            for (name, v) in [("lmu.tyre_compound", 1), ("lmu.tyre_compound_type", 1)] {
                let mut values = vec![v; 3];
                values[0] = if name == "lmu.tyre_compound" {
                    start
                } else {
                    [0, 1, 0, 2][WHEELS.iter().position(|x| x == w).unwrap()]
                };
                t.add_channel(Channel::new(
                    format!("{name}{w}"),
                    Unit::None,
                    ChannelData::I32(values),
                ))
                .unwrap();
            }
        }
        let c = lap_conditions_named(GameId::Lmu, &t, &names)
            .tyre_compound
            .unwrap();
        assert_eq!(c.fl, "Medium");
        let b = c.before.unwrap();
        assert_eq!([b.fl, b.fr, b.rl, b.rr], ["Soft", "Medium", "Soft", "Hard"]);
        assert!(is_conditions_channel(
            GameId::Lmu,
            "lmu.tyre_compound_type_rr"
        ));
    }

    #[test]
    fn tyres_need_all_four_wheels() {
        let t = tel(vec![("f1.tyre_surface_temp_fl", Unit::Celsius, vec![90.0])]);
        assert_eq!(lap_conditions(GameId::F1_25, &t).tyre_temp_c, None);
    }

    #[test]
    fn loads_only_what_it_needs() {
        assert!(is_conditions_channel(GameId::F1_25, "f1.tyre_wear_rl"));
        assert!(is_conditions_channel(GameId::Lmu, names::FUEL));
        assert!(!is_conditions_channel(GameId::Lmu, "f1.tyre_wear_rl"));
        assert!(!is_conditions_channel(GameId::Lmu, "lmu.tyre_temp_x"));
        assert!(!is_conditions_channel(GameId::Iracing, names::SPEED));
    }
}
