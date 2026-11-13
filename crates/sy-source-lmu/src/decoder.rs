// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Turns LMU records ([`crate::records`]) into [`SourceEvent`]s.
//!
//! # Behaviour
//!
//! (Checked with recordings; docs/games/lmu.md.)
//!
//! - One frame per telemetry update of the player's car (`mElapsedTime`
//!   changed), only while driving (`mInRealtime`). Leaving the car mid-lap
//!   aborts the lap.
//! - Back to the garage from the menu: the game puts the car into the
//!   garage (the position jumps) a moment before the scoring says the
//!   player left the car, and may count the lap as completed. The jump
//!   aborts the lap where the car was ("back to the garage"); the game's
//!   lap count for it is ignored.
//! - Races: nothing before the start (formation lap, countdown:
//!   `mGamePhase` < 5). The first lap starts at the green flag
//!   (`mLapStartET`); with a rolling start that is before the line, and
//!   the distance is negative up to the line.
//! - **Lap distance** exists only in the scoring, five times a second. Each
//!   frame's distance is the last scoring distance plus the distance driven
//!   since then (speed × time); the error found at the next scoring update
//!   is corrected over 0.2 s, so the distance stays continuous. At the line
//!   (the telemetry's lap number goes up) the distance restarts from the
//!   lap's start time (`mLapStartET`).
//! - Lap times: when the player's completed laps (`mTotalLaps`) go up, the
//!   scoring's last lap time and sectors are the official times of the lap
//!   that ended. No time (≤ 0) means the game didn't time it: after a
//!   track-limits warning in the lap (the steps went up) the reason is
//!   "track limits"; when the game stopped counting the lap on track
//!   (`mCountLapFlag` from 2 to 1, not back) "off track"; otherwise "not
//!   timed by the game" (out-laps, in-laps outside races).
//! - Validity: also the game's `mLapInvalidated` during the lap (never set
//!   in the recordings so far).
//! - A session is one session number on one track with one car, started at
//!   one time (the elapsed time going back means a restart). The game's
//!   session started at the scoring record's receive time minus the
//!   elapsed time (part of the key); online, where the server's session
//!   may have run for long, Stintory's session starts when the player
//!   joined.
//! - Rain and the track's wetness come from the scoring (`lmu.rain`,
//!   `lmu.track_wetness`).
//! - Keys: session `lmu:<driver>:<start>:<session number>:<vehicle>` (driver
//!   and vehicle as hashes), track `lmu:<track name>:<length>`, car
//!   `lmu:<vehicle model>:<class>`.
//! - Steering: `mPhysicalSteeringWheelRange` is 0 for the cars recorded so
//!   far; the setup's `VM_STEER_LOCK` has the wheel's range.
//! - Axes: the world is left-handed with +y up, so `pos_x` = x, `pos_y` = z
//!   gives an unmirrored map (the game's track map has the same
//!   coordinates). The car's +x points left, +y up, +z back.

use std::collections::VecDeque;

use sy_model::capture::{
    ChannelSpec, Decoder, Frame, RawRecord, SessionMeta, SourceError, SourceEvent, ValueKind,
};
use sy_model::channel::names;
use sy_model::session::Conditions;
use sy_model::{GameId, SessionType, Unit};
use time::OffsetDateTime;

use crate::records::{KIND_SCORING, KIND_SETUP, KIND_TELEMETRY, Scoring, Telemetry};

/// Telemetry samples measured before the session info is sent (sample
/// rate, steering range).
const RATE_SAMPLES: usize = 10;
/// A scoring distance this far off the estimate replaces it (reset to the
/// pits, first update after joining).
const SNAP_M: f64 = 30.0;
/// Corrections of the estimated distance are spread over this time.
const CORRECTION_S: f64 = 0.2;
/// Kelvin to °C.
const KELVIN: f64 = 273.15;
/// `mGamePhase` of a running session (green flag); races are before the
/// start below it (formation lap, countdown).
const PHASE_GREEN: u8 = 5;
/// The car moved this much further between two samples than it could
/// drive: it was put somewhere else (back to the garage).
const JUMP_M: f64 = 20.0;

/// Decoder for LMU records.
#[derive(Debug, Default)]
pub struct LmuDecoder {
    scoring: Option<Scoring>,
    setup: Option<serde_json::Value>,
    /// Steering wheel range of the setup (`VM_STEER_LOCK`), degrees
    setup_steer_lock: f32,
    s: Option<SessionState>,
}

impl LmuDecoder {
    /// Creates a decoder.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Estimates the distance from the line between scoring updates.
#[derive(Debug, Default)]
struct DistEstimate {
    dist: f64,
    /// Elapsed time of `dist`
    time: f64,
    /// Correction still to apply
    correction: f64,
    /// Applied at this rate, m/s
    rate: f64,
    valid: bool,
}

impl DistEstimate {
    /// A scoring update: distance `lap_dist` at elapsed time `et`.
    fn scoring(&mut self, et: f64, lap_dist: f64, track_length: f64, speed: f64) {
        if !self.valid {
            *self = Self {
                dist: lap_dist,
                time: et,
                correction: 0.0,
                rate: 0.0,
                valid: true,
            };
            return;
        }
        let predicted = self.dist + speed * (et - self.time);
        let mut err = lap_dist - predicted;
        if track_length > 0.0 {
            // the scoring and the telemetry cross the line at slightly
            // different times
            if err > track_length / 2.0 {
                err -= track_length;
            } else if err < -track_length / 2.0 {
                err += track_length;
            }
        }
        if err.abs() > SNAP_M {
            self.dist = lap_dist + speed * (self.time - et);
            self.correction = 0.0;
        } else {
            self.correction = err;
            self.rate = err.abs() / CORRECTION_S;
        }
    }

    /// The lap starts before the line (race start): the distance counts
    /// up to 0 at the line instead of up to the track length.
    fn before_line(&mut self, track_length: f64) {
        if track_length > 0.0 && self.dist > track_length / 2.0 {
            self.dist -= track_length;
        }
    }

    /// The line was crossed: the lap started at `lap_start_et`.
    fn lap_started(&mut self, et: f64, lap_start_et: f64, speed: f64) {
        self.dist = speed * (et - lap_start_et).clamp(0.0, 1.0);
        self.time = et;
        self.correction = 0.0;
    }

    /// Distance at elapsed time `et`, driving at `speed`.
    fn advance(&mut self, et: f64, speed: f64) -> f64 {
        let dt = (et - self.time).clamp(0.0, 1.0);
        self.dist += speed * dt;
        let step = self.rate * dt;
        let apply = self.correction.clamp(-step, step);
        self.dist += apply;
        self.correction -= apply;
        self.time = et;
        self.dist
    }
}

#[derive(Debug)]
struct SessionState {
    session: i32,
    track_name: String,
    driver: String,
    /// Wall-clock start of the game's session (receive time − elapsed
    /// time); part of the key, so reconnecting keeps the session
    session_start: OffsetDateTime,
    /// The session's start for Stintory: the game's, offline; online the
    /// server's session may have run for long before, so it is when the
    /// player joined
    started_at: OffsetDateTime,
    game_version: i32,
    last_et: f64,
    /// Vehicle (livery) of the player, from the telemetry
    vehicle: Option<String>,
    /// `mDeltaTime` of the first telemetry samples
    deltas: Vec<f64>,
    /// Steering range when the session info was built
    steering_range: f32,
    conditions: Conditions,
    meta: Option<SessionMeta>,
    est: DistEstimate,
    last_frame_et: Option<f64>,
    /// Speed of the last frame, m/s
    last_speed: f64,
    /// Lap number of the last frame (`None` while not driving)
    frame_lap: Option<i32>,
    /// The running lap started at the line (its distance is ≥ 0)
    from_line: bool,
    /// Position, elapsed time and speed of the last telemetry sample
    last_pos: Option<([f64; 3], f64, f64)>,
    /// The car was put somewhere else: no frames until the next scoring
    jumped: bool,
    /// Frames were skipped because the race hadn't started
    before_start: bool,
    /// Laps that ended in the telemetry and wait for the scoring's time:
    /// (lap, elapsed time at the line, track-limits steps went up)
    untimed: VecDeque<(i32, f64, bool)>,
    /// The last lap that got the scoring's verdict
    last_timed: Option<i32>,
    total_laps: Option<i16>,
    /// The lap the game's invalidation was already reported for
    invalid_reported: Option<i32>,
    /// Track-limits steps at the start of the lap
    track_limits_at_start: u8,
    /// Track-limits steps of the last frame
    last_track_limits: u8,
    /// `mCountLapFlag` of the last scoring
    last_count_flag: Option<u8>,
    /// The game stopped counting the lap (count flag from 2 down) on track
    /// and didn't count it again
    count_dropped: bool,
}

impl SessionState {
    fn new(sc: &Scoring) -> Self {
        let wall =
            OffsetDateTime::from_unix_timestamp_nanos(i128::from(sc.received_unix_ms) * 1_000_000)
                .unwrap_or(OffsetDateTime::UNIX_EPOCH);
        let session_start = wall - time::Duration::seconds_f64(sc.current_et.max(0.0));
        Self {
            session: sc.session,
            track_name: sc.track_name.clone(),
            driver: sc.player.driver_name.clone(),
            session_start,
            started_at: if sc.game_mode == 0 {
                session_start
            } else {
                wall
            },
            game_version: sc.game_version,
            last_et: sc.current_et,
            vehicle: None,
            deltas: Vec::new(),
            steering_range: 0.0,
            conditions: Conditions {
                air_temp_c: Some(sc.ambient_temp_c as f32),
                track_temp_c: Some(sc.track_temp_c as f32),
                weather: Some(weather(sc)),
            },
            meta: None,
            est: DistEstimate::default(),
            last_frame_et: None,
            last_speed: 0.0,
            frame_lap: None,
            from_line: false,
            last_pos: None,
            jumped: false,
            before_start: false,
            untimed: VecDeque::new(),
            last_timed: None,
            total_laps: None,
            invalid_reported: None,
            track_limits_at_start: 0,
            last_track_limits: 0,
            last_count_flag: None,
            count_dropped: false,
        }
    }

    /// Track-limits steps went up in the running lap.
    fn track_limits_in_lap(&self) -> bool {
        self.last_track_limits > self.track_limits_at_start
    }

    fn same_session(&self, sc: &Scoring) -> bool {
        self.session == sc.session
            && self.track_name == sc.track_name
            && self.driver == sc.player.driver_name
            && sc.current_et >= self.last_et - 1.0
    }

    fn key(&self) -> String {
        format!(
            "lmu:{:08x}:{}:{}:{:08x}",
            fnv1a(&self.driver),
            // the start is computed from the receive time; rounding keeps
            // the key when the app reconnects to the same session
            self.session_start.unix_timestamp().div_euclid(10),
            self.session,
            fnv1a(self.vehicle.as_deref().unwrap_or_default()),
        )
    }

    fn build_meta(
        &self,
        sc: &Scoring,
        t: &Telemetry,
        setup: Option<&serde_json::Value>,
    ) -> SessionMeta {
        let model = if t.vehicle_model.is_empty() {
            t.vehicle_name.clone()
        } else {
            t.vehicle_model.clone()
        };
        let class = vehicle_class(t.vehicle_class);
        let car_key = match class {
            Some(c) => format!("lmu:{}:{}", key_part(&model), key_part(c)),
            None => format!("lmu:{}", key_part(&model)),
        };
        let info = serde_json::json!({
            "session": sc.session,
            "session_name": session_name(sc.session),
            "track_length_m": sc.track_length,
            "max_laps": (sc.max_laps > 0 && sc.max_laps < 100_000).then_some(sc.max_laps),
            "session_length_s": (sc.end_et > 0.0).then_some(sc.end_et),
            "online": sc.game_mode != 0,
            "field_size": sc.num_vehicles,
            "fixed_setup": sc.is_fixed_setup,
            "track_grip_level": sc.track_grip_level,
            "track_limits_steps_per_point": sc.track_limits_steps_per_point,
            "track_limits_steps_per_penalty": sc.track_limits_steps_per_penalty,
            "vehicle": t.vehicle_name,
            "vehicle_model": t.vehicle_model,
            "vehicle_class": class,
            "vehicle_class_id": t.vehicle_class,
            "championship": championship(t.vehicle_championship),
            "championship_id": t.vehicle_championship,
            "steering_range_deg": (self.steering_range > 0.0).then_some(self.steering_range),
            "max_gears": t.max_gears,
            "fuel_capacity_l": t.fuel_capacity,
            "tyres_front": t.front_compound,
            "tyres_rear": t.rear_compound,
            "tyre_compounds_front": { t.front_compound_index.to_string(): t.front_compound },
            "tyre_compounds_rear": { t.rear_compound_index.to_string(): t.rear_compound },
        });
        SessionMeta {
            session_key: self.key(),
            game: GameId::Lmu,
            game_version: (self.game_version != 0).then(|| self.game_version.to_string()),
            track_key: track_key(&sc.track_name, sc.track_length),
            track_name: track_name(&sc.track_name, sc.track_length),
            car_key,
            car_name: model,
            session_type: session_type(sc.session),
            started_at: self.started_at,
            conditions: self.conditions.clone(),
            setup: setup.cloned(),
            session_info: Some(info),
            sample_rate_hz: sample_rate(&self.deltas),
            channels: channels(self.steering_range > 0.0)
                .into_iter()
                .map(|c| c.spec)
                .collect(),
        }
    }
}

/// FNV-1a, 32 bit: a stable short hash for keys.
fn fnv1a(s: &str) -> u32 {
    s.bytes().fold(0x811c_9dc5u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    })
}

/// Lower case, letters and digits, `_` between words.
fn key_part(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        "unknown".into()
    } else {
        out
    }
}

/// Track key: the game's track name with the layout's length (`mLapDist`
/// of the scoring, whole metres). The name of a venue's main layout is the
/// venue ("Autodromo Nazionale Monza"); other layouts may have their own
/// ("Monza Curva Grande Circuit") or not, so the length tells them apart.
pub fn track_key(name: &str, length_m: f64) -> String {
    if length_m > 0.0 {
        format!("lmu:{}:{}", key_part(name), length_m.round() as i64)
    } else {
        format!("lmu:{}", key_part(name))
    }
}

/// Track name with the layout's length, e.g. "Autodromo Nazionale Monza
/// (5.78 km)".
pub fn track_name(name: &str, length_m: f64) -> String {
    if length_m > 0.0 {
        format!("{name} ({:.2} km)", length_m / 1000.0)
    } else {
        name.to_owned()
    }
}

/// The setup from `getPlayerGarageData`, reduced to what the setup is:
/// `{"VM_BRAKE_BALANCE": "51.2:48.8", …}`. The game's response has one
/// object per setting with the readable `stringValue`, the index `value`,
/// limits, and UI state (`numChangesValue`, `lastSavedStringValue`, …) that
/// changes without the setup changing. Settings the car doesn't have
/// (`available` false, "N/A") are left out, as is `gearGraph` (derived from
/// the gears). A response in another format is kept as it is.
pub fn compact_setup(garage: &serde_json::Value) -> serde_json::Value {
    let Some(map) = garage.as_object() else {
        return garage.clone();
    };
    let mut out = serde_json::Map::new();
    let mut settings = 0;
    for (key, v) in map {
        match v {
            serde_json::Value::Object(o) => {
                let Some(text) = o.get("stringValue").and_then(|t| t.as_str()) else {
                    continue;
                };
                settings += 1;
                let available = o.get("available").and_then(|a| a.as_bool()) != Some(false);
                let text = text.trim();
                if available && !text.is_empty() && text != "N/A" {
                    out.insert(key.clone(), text.into());
                }
            }
            serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_) => {
                out.insert(key.clone(), v.clone());
            }
            _ => {}
        }
    }
    if settings == 0 {
        return garage.clone();
    }
    serde_json::Value::Object(out)
}

/// Session type of `mSession` (InternalsPlugin.hpp: 0 test day,
/// 1–4 practice, 5–8 qualifying, 9 warm-up, 10–13 race).
pub fn session_type(session: i32) -> SessionType {
    match session {
        0..=4 | 9 => SessionType::Practice,
        5..=8 => SessionType::Qualifying,
        10..=13 => SessionType::Race,
        _ => SessionType::Other,
    }
}

/// Name of `mSession`.
pub fn session_name(session: i32) -> String {
    match session {
        0 => "Test day".into(),
        1..=4 => format!("Practice {session}"),
        5..=8 => format!("Qualifying {}", session - 4),
        9 => "Warm-up".into(),
        10..=13 => format!("Race {}", session - 9),
        _ => format!("Session {session}"),
    }
}

/// Name of `IP_VehicleClass`.
pub fn vehicle_class(id: u8) -> Option<&'static str> {
    Some(match id {
        0x00 => "Hypercar",
        0x02 => "LMP2 ELMS",
        0x03 => "LMP2",
        0x04 => "LMP3",
        0x05 => "GTE",
        0x06 => "GT3",
        0x08 => "Pace car",
        _ => return None,
    })
}

/// Name of `IP_VehicleChampionship`.
pub fn championship(id: u8) -> Option<&'static str> {
    Some(match id {
        0x00 => "WEC 2023",
        0x01 => "WEC 2024",
        0x02 => "WEC 2025",
        0x03 => "WEC 2026",
        0x10 => "ELMS 2025",
        0x11 => "ELMS 2026",
        _ => return None,
    })
}

/// Adds the compound names of the fitted tyres to the session info's
/// index → name tables; true if one was new.
fn add_compound_names(meta: &mut SessionMeta, t: &Telemetry) -> bool {
    let Some(serde_json::Value::Object(info)) = meta.session_info.as_mut() else {
        return false;
    };
    let mut added = false;
    for (key, index, name) in [
        (
            "tyre_compounds_front",
            t.front_compound_index,
            &t.front_compound,
        ),
        (
            "tyre_compounds_rear",
            t.rear_compound_index,
            &t.rear_compound,
        ),
    ] {
        if name.is_empty() {
            continue;
        }
        let table = info
            .entry(key)
            .or_insert_with(|| serde_json::Value::Object(Default::default()));
        if let serde_json::Value::Object(table) = table
            && !table.contains_key(&index.to_string())
        {
            table.insert(index.to_string(), name.clone().into());
            added = true;
        }
    }
    added
}

/// °C from K; NaN for 0 K and below (no value from the game).
fn kelvin_to_celsius(k: f64) -> f64 {
    if k > 0.0 { k - KELVIN } else { f64::NAN }
}

fn weather(sc: &Scoring) -> String {
    let sky = if sc.raining >= 0.05 {
        format!("rain {:.0} %", sc.raining * 100.0)
    } else if sc.dark_cloud >= 0.5 {
        "dry, cloudy".into()
    } else {
        "dry".into()
    };
    if sc.avg_path_wetness >= 0.05 {
        format!("{sky}, track wet {:.0} %", sc.avg_path_wetness * 100.0)
    } else {
        sky
    }
}

/// Nominal rate from the game's time steps, rounded to whole Hz.
fn sample_rate(deltas: &[f64]) -> f32 {
    let mut d: Vec<f64> = deltas.iter().copied().filter(|d| *d > 0.0).collect();
    if d.is_empty() {
        return 50.0;
    }
    d.sort_by(f64::total_cmp);
    (1.0 / d[d.len() / 2]).round() as f32
}

/// Our heading (clockwise from `pos_y`, −π…π): the car's forward direction
/// (local −z) in world x/z.
fn heading(t: &Telemetry) -> f64 {
    let fx = -t.ori[0][2];
    let fz = -t.ori[2][2];
    fx.atan2(fz)
}

/// One channel and how to compute it.
struct Ch {
    spec: ChannelSpec,
    get: Get,
}

/// Where a channel's value comes from.
enum Get {
    Telemetry(fn(&Telemetry) -> f64),
    /// The last scoring (5 Hz)
    Scoring(fn(&Scoring) -> f64),
}

impl Ch {
    fn value(&self, t: &Telemetry, sc: &Scoring) -> f64 {
        match self.get {
            Get::Telemetry(f) => f(t),
            Get::Scoring(f) => f(sc),
        }
    }
}

fn ch(name: impl Into<String>, unit: Unit, kind: ValueKind, get: fn(&Telemetry) -> f64) -> Ch {
    Ch {
        spec: ChannelSpec::new(name, unit, kind),
        get: Get::Telemetry(get),
    }
}

fn ch_scoring(name: &str, unit: Unit, kind: ValueKind, get: fn(&Scoring) -> f64) -> Ch {
    Ch {
        spec: ChannelSpec::new(name, unit, kind),
        get: Get::Scoring(get),
    }
}

fn flag(b: bool) -> f64 {
    f64::from(u8::from(b))
}

/// The channels, in frame order. `steer` only when the game reports the
/// steering wheel range.
fn channels(with_steer: bool) -> Vec<Ch> {
    use ValueKind::{Bool, F32, I32};
    let mut v = vec![
        ch(names::SPEED, Unit::MeterPerSecond, F32, Telemetry::speed),
        ch(names::THROTTLE, Unit::Ratio, F32, |t| t.throttle),
        ch(names::BRAKE, Unit::Ratio, F32, |t| t.brake),
        ch(names::CLUTCH, Unit::Ratio, F32, |t| t.clutch),
        ch(names::STEER_NORM, Unit::Ratio, F32, |t| t.steering),
    ];
    if with_steer {
        v.push(ch(names::STEER, Unit::Radian, F32, |t| {
            if t.steering_range_deg > 0.0 {
                t.steering * f64::from(t.steering_range_deg).to_radians() / 2.0
            } else {
                f64::NAN
            }
        }));
    }
    v.extend([
        ch(names::GEAR, Unit::None, I32, |t| f64::from(t.gear)),
        ch(names::RPM, Unit::Rpm, F32, |t| t.engine_rpm),
        // left-handed world, +y up: x/z seen from above
        ch(names::POS_X, Unit::Meter, F32, |t| t.pos[0]),
        ch(names::POS_Y, Unit::Meter, F32, |t| t.pos[2]),
        ch(names::POS_Z, Unit::Meter, F32, |t| t.pos[1]),
        // car coordinates: +x left, +y up, +z back
        ch(names::G_LAT, Unit::MeterPerSecondSquared, F32, |t| {
            -t.local_accel[0]
        }),
        ch(names::G_LONG, Unit::MeterPerSecondSquared, F32, |t| {
            -t.local_accel[2]
        }),
        ch(names::G_VERT, Unit::MeterPerSecondSquared, F32, |t| {
            t.local_accel[1]
        }),
        ch(names::YAW, Unit::Radian, F32, heading),
        // +y yaws to the right (clockwise, like our heading)
        ch(names::YAW_RATE, Unit::RadianPerSecond, F32, |t| {
            t.local_rot[1]
        }),
        ch(names::FUEL, Unit::Liter, F32, |t| t.fuel),
        ch(names::BRAKE_BIAS, Unit::Ratio, F32, |t| {
            1.0 - t.rear_brake_bias
        }),
        ch(names::ABS_ACTIVE, Unit::None, Bool, |t| flag(t.abs_active)),
        ch(names::TC_ACTIVE, Unit::None, Bool, |t| flag(t.tc_active)),
        ch("lmu.virtual_energy", Unit::Ratio, F32, |t| {
            f64::from(t.virtual_energy)
        }),
        ch("lmu.battery", Unit::Ratio, F32, |t| t.battery),
        ch("lmu.regen", Unit::Watt, F32, |t| {
            f64::from(t.regen_kw) * 1000.0
        }),
        ch("lmu.motor_torque", Unit::NewtonMeter, F32, |t| {
            t.motor_torque
        }),
        ch("lmu.motor_rpm", Unit::Rpm, F32, |t| t.motor_rpm),
        ch("lmu.motor_state", Unit::None, I32, |t| {
            f64::from(t.motor_state)
        }),
        ch("lmu.tc_level", Unit::None, I32, |t| f64::from(t.tc)),
        ch("lmu.tc_slip", Unit::None, I32, |t| f64::from(t.tc_slip)),
        ch("lmu.tc_cut", Unit::None, I32, |t| f64::from(t.tc_cut)),
        ch("lmu.abs_level", Unit::None, I32, |t| f64::from(t.abs)),
        ch("lmu.motor_map", Unit::None, I32, |t| f64::from(t.motor_map)),
        ch("lmu.migration", Unit::None, I32, |t| f64::from(t.migration)),
        ch("lmu.front_arb", Unit::None, I32, |t| f64::from(t.front_arb)),
        ch("lmu.rear_arb", Unit::None, I32, |t| f64::from(t.rear_arb)),
        ch("lmu.lift_and_coast", Unit::None, I32, |t| {
            f64::from(t.lift_and_coast)
        }),
        ch("lmu.track_limits_steps", Unit::None, I32, |t| {
            f64::from(t.track_limits_steps)
        }),
        ch("lmu.lap_invalidated", Unit::None, Bool, |t| {
            flag(t.lap_invalidated)
        }),
        ch("lmu.speed_limiter", Unit::None, Bool, |t| {
            flag(t.speed_limiter)
        }),
        ch("lmu.engine_torque", Unit::NewtonMeter, F32, |t| {
            t.engine_torque
        }),
        ch("lmu.water_temp", Unit::Celsius, F32, |t| t.water_temp_c),
        ch("lmu.oil_temp", Unit::Celsius, F32, |t| t.oil_temp_c),
        ch("lmu.ride_height_front", Unit::Meter, F32, |t| {
            t.front_ride_height
        }),
        ch("lmu.ride_height_rear", Unit::Meter, F32, |t| {
            t.rear_ride_height
        }),
        ch("lmu.downforce_front", Unit::Newton, F32, |t| {
            t.front_downforce
        }),
        ch("lmu.downforce_rear", Unit::Newton, F32, |t| {
            t.rear_downforce
        }),
        // tyre compound per axle: an index, the names are in the session info
        // (`tyre_compounds_front` / `_rear`: index → name)
        ch("lmu.tyre_compound_front", Unit::None, I32, |t| {
            f64::from(t.front_compound_index)
        }),
        ch("lmu.tyre_compound_rear", Unit::None, I32, |t| {
            f64::from(t.rear_compound_index)
        }),
        // weather (per lap in the lap table)
        ch_scoring(names::AIR_TEMP, Unit::Celsius, F32, |s| s.ambient_temp_c),
        ch_scoring(names::TRACK_TEMP, Unit::Celsius, F32, |s| s.track_temp_c),
        ch_scoring("lmu.rain", Unit::Ratio, F32, |s| s.raining),
        ch_scoring("lmu.track_wetness", Unit::Ratio, F32, |s| {
            s.avg_path_wetness
        }),
    ]);
    // per wheel; closures can't capture the index, so one entry per wheel
    // (the game's order: front left, front right, rear left, rear right)
    macro_rules! wheels {
        ($prefix:literal, $unit:expr, $kind:expr, |$w:ident| $e:expr) => {
            v.push(ch(concat!($prefix, "_fl"), $unit, $kind, |t| {
                let $w = &t.wheels[0];
                $e
            }));
            v.push(ch(concat!($prefix, "_fr"), $unit, $kind, |t| {
                let $w = &t.wheels[1];
                $e
            }));
            v.push(ch(concat!($prefix, "_rl"), $unit, $kind, |t| {
                let $w = &t.wheels[2];
                $e
            }));
            v.push(ch(concat!($prefix, "_rr"), $unit, $kind, |t| {
                let $w = &t.wheels[3];
                $e
            }));
        };
    }
    // 0 K: no value (the game zeroes the wheels when going back to the garage)
    wheels!("lmu.tyre_temp", Unit::Celsius, F32, |w| {
        kelvin_to_celsius(w.temperature_k.iter().sum::<f64>() / 3.0)
    });
    wheels!("lmu.tyre_carcass_temp", Unit::Celsius, F32, |w| {
        kelvin_to_celsius(w.carcass_temp_k)
    });
    wheels!("lmu.tyre_pressure", Unit::Kilopascal, F32, |w| w
        .pressure_kpa);
    // the game's mWear is what is left (1 new)
    wheels!("lmu.tyre_wear", Unit::Ratio, F32, |w| 1.0 - w.wear);
    wheels!("lmu.tyre_flat", Unit::None, Bool, |w| f64::from(u8::from(
        w.flat
    )));
    wheels!("lmu.wheel_detached", Unit::None, Bool, |w| f64::from(
        u8::from(w.detached)
    ));
    wheels!("lmu.brake_temp", Unit::Celsius, F32, |w| w.brake_temp_c);
    wheels!("lmu.ride_height", Unit::Meter, F32, |w| w.ride_height);
    wheels!("lmu.suspension_deflection", Unit::Meter, F32, |w| {
        w.suspension_deflection
    });
    // 0 dry, 1 wet, 2 grass, 3 dirt, 4 gravel, 5 rumble strip, 6 special
    wheels!("lmu.surface", Unit::None, I32, |w| f64::from(
        w.surface_type
    ));
    v
}

impl LmuDecoder {
    fn on_scoring(&mut self, sc: Scoring, out: &mut Vec<SourceEvent>) {
        if let Some(s) = &self.s
            && !s.same_session(&sc)
        {
            if s.meta.is_some() {
                out.push(SourceEvent::SessionEnd);
            }
            self.s = None;
        }
        let s = self.s.get_or_insert_with(|| SessionState::new(&sc));
        s.last_et = sc.current_et;
        s.jumped = false;

        if !sc.in_realtime {
            if s.frame_lap.take().is_some() && s.meta.is_some() {
                out.push(SourceEvent::LapAborted {
                    reason: "left the car".into(),
                });
            }
            s.est.valid = false;
        } else {
            s.est.scoring(
                sc.current_et,
                sc.player.lap_dist,
                sc.track_length,
                s.last_speed,
            );
        }

        // the scoring counts a completed lap: its official time
        let laps = sc.player.total_laps;
        if let Some(prev) = s.total_laps
            && laps > prev
            && s.meta.is_some()
        {
            while s
                .untimed
                .front()
                .is_some_and(|&(_, end, _)| end < sc.current_et - 30.0)
            {
                s.untimed.pop_front();
            }
            let lap = match s.untimed.front() {
                Some(&(lap, end, track_limits)) if end <= sc.current_et + 0.5 => {
                    s.untimed.pop_front();
                    Some((lap, track_limits))
                }
                // the scoring was first; the telemetry's line follows
                _ => s.frame_lap.map(|l| (l, s.track_limits_in_lap())),
            };
            if let Some((lap, track_limits)) = lap
                && s.last_timed.is_none_or(|t| lap > t)
            {
                let why = if track_limits {
                    "track limits"
                } else if s.count_dropped {
                    "off track"
                } else {
                    "not timed by the game"
                };
                lap_time(&sc, lap, why, out);
                s.last_timed = Some(lap);
            }
            s.count_dropped = false;
        }
        s.total_laps = Some(laps);
        // off the track the game stops counting the lap (1 = lap only); it
        // counts again (2) when it forgives it. Out-laps and the pit lane
        // are 1 anyway.
        let count = sc.player.count_lap_flag;
        if count >= 2 {
            s.count_dropped = false;
        } else if sc.in_realtime && !sc.player.in_pits && s.last_count_flag == Some(2) {
            s.count_dropped = true;
        }
        s.last_count_flag = Some(count);
        self.scoring = Some(sc);
    }

    fn on_telemetry(&mut self, t: Telemetry, out: &mut Vec<SourceEvent>) {
        let Some(sc) = self.scoring.as_ref() else {
            return;
        };
        if !sc.in_realtime {
            return;
        }
        let setup_steer_lock = self.setup_steer_lock;
        let Some(s) = self.s.as_mut() else { return };
        let et = t.elapsed_time;
        if s.last_frame_et.is_some_and(|last| et <= last) {
            return; // paused or repeated
        }
        s.last_frame_et = Some(et);
        s.last_speed = t.speed();

        // another car: a new session
        if s.vehicle.as_ref().is_some_and(|v| *v != t.vehicle_name) {
            if s.meta.is_some() {
                out.push(SourceEvent::SessionEnd);
            }
            let mut fresh = SessionState::new(sc);
            fresh.started_at = s.started_at;
            fresh.session_start = s.session_start;
            fresh.est = std::mem::take(&mut s.est);
            fresh.total_laps = s.total_laps;
            fresh.last_frame_et = s.last_frame_et;
            fresh.last_speed = s.last_speed;
            *s = fresh;
        }
        s.vehicle = Some(t.vehicle_name.clone());

        if s.deltas.len() < 32 {
            s.deltas.push(t.delta_time);
        }
        let speed = t.speed();

        // Back to the garage from the menu: the car is put into the garage
        // a moment before the scoring says it left the car (and the game
        // may count the lap as completed). The lap ends where the car was.
        let prev_pos = s.last_pos.replace((t.pos, et, speed));
        if let Some((pos, at, prev_speed)) = prev_pos {
            let moved = pos
                .iter()
                .zip(t.pos)
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f64>()
                .sqrt();
            let could = speed.max(prev_speed) * (et - at).max(0.0) * 2.0;
            if moved > JUMP_M + could {
                if let Some(lap) = s.frame_lap.take() {
                    out.push(SourceEvent::LapAborted {
                        reason: if t.in_pit_lane() {
                            "back to the garage".into()
                        } else {
                            "car reset".into()
                        },
                    });
                    s.last_timed = s.last_timed.max(Some(lap));
                }
                s.est.valid = false;
                s.jumped = true;
                s.count_dropped = false;
            }
        }
        if s.jumped {
            return; // until the scoring catches up
        }

        if s.meta.is_none() {
            if s.deltas.len() < RATE_SAMPLES {
                s.est.advance(et, speed);
                return;
            }
            s.steering_range = if t.steering_range_deg > 0.0 {
                t.steering_range_deg
            } else {
                setup_steer_lock
            };
            let meta = s.build_meta(sc, &t, self.setup.as_ref());
            s.meta = Some(meta.clone());
            out.push(SourceEvent::Session(Box::new(meta)));
        } else if let Some(meta) = s.meta.as_mut()
            && add_compound_names(meta, &t)
        {
            // new tyres (pit stop): their name for the lap table
            out.push(SourceEvent::Session(Box::new(meta.clone())));
        }

        // Races: nothing before the start (formation lap, countdown). The
        // first lap starts at the green flag (`mLapStartET`), with a
        // rolling start before the line.
        if session_type(sc.session) == SessionType::Race
            && sc.game_phase < PHASE_GREEN
            && (t.lap_start_et <= 0.0 || t.lap_start_et + 0.5 < sc.current_et)
        {
            if s.frame_lap.take().is_some() {
                out.push(SourceEvent::LapAborted {
                    reason: "before the start".into(),
                });
            }
            if s.est.valid {
                s.est.advance(et, speed);
            }
            s.before_start = true;
            return;
        }
        if std::mem::take(&mut s.before_start) {
            s.est.before_line(sc.track_length);
        }

        let lap = t.lap_number;
        match s.frame_lap {
            Some(prev) if lap > prev => {
                // unless the scoring already timed it (it was first)
                if s.last_timed.is_none_or(|t| prev > t) {
                    s.untimed.push_back((prev, et, s.track_limits_in_lap()));
                }
                s.est.lap_started(et, t.lap_start_et, speed);
                s.track_limits_at_start = t.track_limits_steps;
                s.from_line = true;
            }
            Some(_) => {}
            None => {
                s.track_limits_at_start = t.track_limits_steps;
                s.from_line = false;
            }
        }
        s.frame_lap = Some(lap);
        s.last_track_limits = t.track_limits_steps;
        let dist = if s.est.valid {
            s.est.advance(et, speed)
        } else {
            0.0
        };

        // the game reports no range for some cars; the setup has it
        let mut t = t;
        if t.steering_range_deg <= 0.0 {
            t.steering_range_deg = setup_steer_lock;
        }
        if t.lap_invalidated && s.invalid_reported != Some(lap) {
            s.invalid_reported = Some(lap);
            let reason = if t.track_limits_steps > s.track_limits_at_start {
                "track limits"
            } else {
                "invalidated by the game"
            };
            out.push(SourceEvent::Invalidate {
                lap: Some(lap),
                reason: reason.into(),
            });
        }

        let values = channels(s.steering_range > 0.0)
            .iter()
            .map(|c| c.value(&t, sc))
            .collect();
        out.push(SourceEvent::Frame(Frame {
            session_time: et,
            lap,
            // before the line (garage, race start) only when the lap didn't
            // start at the line
            lap_dist: if s.from_line { dist.max(0.0) } else { dist } as f32,
            in_pits: t.in_pit_lane() || sc.player.in_pits,
            standing_start: false,
            values,
        }));
    }

    fn on_setup(&mut self, payload: &[u8], out: &mut Vec<SourceEvent>) {
        let Ok(garage) = serde_json::from_slice::<serde_json::Value>(payload) else {
            tracing::debug!("LMU setup is not JSON; ignored");
            return;
        };
        let setup = compact_setup(&garage);
        if self.setup.as_ref() == Some(&setup) {
            return;
        }
        self.setup_steer_lock = steer_lock(&setup).unwrap_or(0.0);
        self.setup = Some(setup.clone());
        if let Some(meta) = self.s.as_mut().and_then(|s| s.meta.as_mut()) {
            meta.setup = Some(setup);
            out.push(SourceEvent::Session(Box::new(meta.clone())));
        }
    }
}

/// The steering wheel range of a compact setup (`VM_STEER_LOCK`, e.g.
/// "516 deg(19.7 )" or "435 (17.7) deg": the wheel's range, then the front
/// wheels' lock), degrees.
fn steer_lock(setup: &serde_json::Value) -> Option<f32> {
    let text = setup.get("VM_STEER_LOCK")?.as_str()?;
    let number: String = text
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    number
        .parse::<f32>()
        .ok()
        .filter(|d| (90.0..=2000.0).contains(d))
}

/// The official time of `lap` from the scoring, or why there is none
/// (`untimed`: track limits, off track, …).
fn lap_time(sc: &Scoring, lap: i32, untimed: &str, out: &mut Vec<SourceEvent>) {
    let p = &sc.player;
    if p.last_lap_time <= 0.0 {
        out.push(SourceEvent::Invalidate {
            lap: Some(lap),
            reason: untimed.into(),
        });
        return;
    }
    let ms = |s: f64| (s * 1000.0).round() as u32;
    let total = ms(p.last_lap_time);
    let (s1, s12) = (p.last_sector1, p.last_sector2);
    let sectors = if s1 > 0.0 && s12 > s1 && p.last_lap_time > s12 {
        let (a, b) = (ms(s1), ms(s12));
        vec![a, b - a, total - b]
    } else {
        Vec::new()
    };
    out.push(SourceEvent::LapTime {
        lap,
        lap_time_ms: total,
        sector_times_ms: sectors,
    });
}

impl Decoder for LmuDecoder {
    fn decode(&mut self, record: &RawRecord) -> Result<Vec<SourceEvent>, SourceError> {
        let mut out = Vec::new();
        match record.kind {
            KIND_TELEMETRY => self.on_telemetry(Telemetry::parse(&record.payload)?, &mut out),
            KIND_SCORING => self.on_scoring(Scoring::parse(&record.payload)?, &mut out),
            KIND_SETUP => self.on_setup(&record.payload, &mut out),
            other => {
                return Err(SourceError::InvalidData(format!(
                    "unknown LMU record kind {other}"
                )));
            }
        }
        Ok(out)
    }

    fn finish(&mut self) -> Vec<SourceEvent> {
        match self.s.take() {
            Some(s) if s.meta.is_some() => vec![SourceEvent::SessionEnd],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_normalised() {
        assert_eq!(key_part("Circuit de la Sarthe"), "circuit_de_la_sarthe");
        assert_eq!(key_part("  Porsche 963 (LMDh) "), "porsche_963_lmdh");
        assert_eq!(key_part("--"), "unknown");
    }

    #[test]
    fn layouts_get_their_own_track_key() {
        // layouts with the same track name
        let name = "Bahrain International Circuit";
        assert_eq!(
            track_key(name, 5411.7),
            "lmu:bahrain_international_circuit:5412"
        );
        assert_ne!(track_key(name, 5411.7), track_key(name, 3543.2));
        assert_eq!(
            track_name("Autodromo Nazionale Monza", 5780.64),
            "Autodromo Nazionale Monza (5.78 km)"
        );
        assert_eq!(track_key("X", 0.0), "lmu:x");
    }

    #[test]
    fn setup_is_compacted() {
        // the shape of getPlayerGarageData (values made up)
        let garage = serde_json::json!({
            "VM_BRAKE_BALANCE": {"available": true, "key": "VM_BRAKE_BALANCE", "stringValue": "51.2:48.8",
                "lastSavedStringValue": "57.0:43.0", "numChangesValue": 3, "value": 23, "minValue": 0, "maxValue": 57},
            "VM_STEER_LOCK": {"available": true, "caption": "Wheel Range (Lock)", "stringValue": "516 deg(19.7 )", "value": 6},
            "VM_CHASSIS_ADJ_00": {"available": true, "stringValue": "N/A", "value": 0},
            "VM_DIFF_PUMP": {"available": false, "stringValue": "10", "value": 1},
            "gearGraph": {"kiloRPM": [7.5, 5.7], "numForwardGears": 6},
            "symmetric": true
        });
        assert_eq!(
            compact_setup(&garage),
            serde_json::json!({
                "VM_BRAKE_BALANCE": "51.2:48.8",
                "VM_STEER_LOCK": "516 deg(19.7 )",
                "symmetric": true
            })
        );
        // UI state alone is no change
        let mut clicked = garage.clone();
        clicked["VM_BRAKE_BALANCE"]["numChangesValue"] = 4.into();
        assert_eq!(compact_setup(&clicked), compact_setup(&garage));
        // unknown formats stay as they are
        let other = serde_json::json!({"setup": [1, 2]});
        assert_eq!(compact_setup(&other), other);
    }

    #[test]
    fn session_numbers() {
        assert_eq!(session_type(0), SessionType::Practice);
        assert_eq!(session_type(6), SessionType::Qualifying);
        assert_eq!(session_type(9), SessionType::Practice);
        assert_eq!(session_type(10), SessionType::Race);
        assert_eq!(session_name(10), "Race 1");
        assert_eq!(session_name(5), "Qualifying 1");
    }

    #[test]
    fn distance_estimate_stays_continuous() {
        let mut e = DistEstimate::default();
        e.scoring(10.0, 100.0, 1000.0, 50.0);
        // 50 m/s for 0.1 s
        assert!((e.advance(10.1, 50.0) - 105.0).abs() < 1e-9);
        // the scoring says 1 m further than estimated at 10.2 s
        e.scoring(10.2, 111.0, 1000.0, 50.0);
        let d1 = e.advance(10.2, 50.0);
        assert!((d1 - 110.5).abs() < 1e-9, "{d1}"); // half of the correction
        let d2 = e.advance(10.3, 50.0);
        assert!((d2 - 116.0).abs() < 1e-9, "{d2}"); // all of it
        let d3 = e.advance(10.5, 50.0);
        assert!((d3 - 126.0).abs() < 1e-9, "{d3}"); // all of it
        // across the line: the scoring has wrapped, the estimate not yet
        e.est_at(990.0, 11.0);
        e.scoring(11.1, 4.0, 1000.0, 50.0);
        assert!((e.correction - 9.0).abs() < 1e-9, "{}", e.correction);
        // far off (reset to the pits): replaced
        e.scoring(11.2, 500.0, 1000.0, 50.0);
        assert!((e.advance(11.2, 50.0) - 500.0).abs() < 1e-9);
    }

    impl DistEstimate {
        fn est_at(&mut self, dist: f64, time: f64) {
            self.dist = dist;
            self.time = time;
            self.correction = 0.0;
        }
    }

    #[test]
    fn heading_is_clockwise_from_pos_y() {
        // identity: the car looks along -z (forward = -z)
        let mut t = Telemetry {
            ori: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            ..Telemetry::default()
        };
        assert!((heading(&t).abs() - std::f64::consts::PI).abs() < 1e-9);
        // car's back (+z local) points to world -z: forward is +z (pos_y)
        t.ori = [[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]];
        assert!(heading(&t).abs() < 1e-9);
    }
}
