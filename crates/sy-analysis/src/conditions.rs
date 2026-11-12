// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Weather, fuel and tyres of one lap, from its telemetry (the lap table's
//! extra columns). Computed when a session is shown, so laps recorded before
//! a game adapter recorded a value simply have none.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use sy_model::api::{LapConditions, WheelFlags, Wheels};
use sy_model::channel::names;
use sy_model::{BlobHash, Channel, ChannelData, GameId, LapTelemetry, Unit};

/// Results kept by telemetry file; a lap's file never changes.
type Cache = Mutex<HashMap<(GameId, BlobHash), LapConditions>>;

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
    hashes: &[&BlobHash],
    mut load: impl FnMut(&BlobHash) -> Result<Vec<u8>, E>,
) -> Vec<Result<LapConditions, String>> {
    let mut out: Vec<Option<Result<LapConditions, String>>> = {
        let cache = cache().lock().unwrap_or_else(|e| e.into_inner());
        hashes
            .iter()
            .map(|h| cache.get(&(game, (*h).clone())).cloned().map(Ok))
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
                                t.map(|t| lap_conditions(game, &t))
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
                cache.insert((game, hashes[*i].clone()), c.clone());
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
}

/// Weather, fuel and tyres of one lap.
pub fn lap_conditions(game: GameId, t: &LapTelemetry) -> LapConditions {
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
        tyre_temp_c: wheels(t, tyre_temp_prefix(game), |c| average(finite(c))),
        tyre_wear: tyre_wear_prefix(game).and_then(|p| wear(game, t, p)),
        tyre_flat: flats(game, t),
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
        let first = laps_conditions(GameId::Lmu, &[&hash, &missing], &mut load);
        assert_eq!(first[0].as_ref().unwrap().fuel_used, Some(0.5));
        assert_eq!(first[1].as_ref().unwrap_err(), "no such file");
        let again = laps_conditions(GameId::Lmu, &[&hash], &mut load);
        assert_eq!(again[0].as_ref().unwrap().fuel_start, Some(12.0));
        assert_eq!(loads, 2, "the second time comes from the cache");
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
