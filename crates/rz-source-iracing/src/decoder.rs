// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Turns iRacing raw records into [`SourceEvent`]s.
//!
//! # Raw records
//!
//! | kind | payload |
//! |---|---|
//! | [`KIND_VAR_TABLE`] | `tick_rate u32 LE`, `row_len u32 LE`, `num_vars u32 LE`, var headers (144 bytes each, SDK layout) |
//! | [`KIND_SESSION_INFO`] | JSON of the player-only [`SessionInfo`] |
//! | [`KIND_DATA`] | one data row (SDK layout), other cars' `CarIdx*` entries zeroed |
//!
//! # Behaviour (verified with recorded `.ibt` files)
//!
//! - `Lap` is the lap being driven; it increments at the line together with
//!   `LapCompleted`. The official time (`LapLastLapTime`) follows 1–2 s later
//!   for lap `LapCompleted`; a change to `-1` means iRacing invalidated it. An
//!   unchanged value carries no information.
//! - Rows with `IsOnTrack == false` or `PlayerTrackSurface == -1`
//!   (not in world) are not driving data and are skipped. The first row of a
//!   file is such a stale row.
//! - iRacing has no "lap invalid" flag while driving; we invalidate a lap on
//!   an incident-count increase or when the car goes off track.
//! - Laps driven on track under the meatball (`irsdk_repair`), black or
//!   disqualification flag don't count in iRacing (`LapLastLapTime` stays
//!   `-1`, so there is no change to see). We invalidate them from
//!   `SessionFlags`. Time in the pit lane doesn't count: there the problem is
//!   being fixed, and iRacing counts the out-lap.
//! - Sign conventions: `SteeringWheelAngle`, `LatAccel` and `YawRate` are
//!   positive to the **left** in iRacing (checked against the GPS heading);
//!   our canonical channels are positive to the right, so they are negated.
//! - Position: `Lat`/`Lon`/`Alt` exist only in `.ibt` files, not in the live
//!   telemetry. `WeekendInfo.TrackLatitude/TrackLongitude` is the
//!   start/finish line. Without GPS we integrate the car's velocity
//!   (`VelocityX` forward, `VelocityY` left, car frame) rotated by
//!   `YawNorth` (heading from north, clockwise):
//!   `east += (vx·sin h − vy·cos h)·dt`, `north += (vx·cos h + vy·sin h)·dt`.
//!   Checked against GPS on a 7-minute `.ibt`: < 2 m apart. The position is
//!   set to the line (0, 0) at each line crossing on track, which keeps the
//!   drift per lap small and puts it in the same frame as the GPS position.
//!   Before the first crossing (out of the pits, joined mid-lap, after a
//!   tow) there is no position.

use rz_model::capture::{
    ChannelSpec, Decoder, Frame, RawRecord, SessionMeta, SourceError, SourceEvent, ValueKind,
};
use rz_model::channel::names;
use rz_model::session::Conditions;
use rz_model::{GameId, SessionType, Unit};
use time::OffsetDateTime;

use crate::format::{VAR_HEADER_LEN, VarHeader, VarTable};
use crate::session_info::SessionInfo;

/// Raw record: variable table.
pub const KIND_VAR_TABLE: u8 = 1;
/// Raw record: player-only session info (JSON).
pub const KIND_SESSION_INFO: u8 = 2;
/// Raw record: one data row.
pub const KIND_DATA: u8 = 3;

/// Earth radius for the local track projection (mean radius, metres).
const EARTH_RADIUS_M: f64 = 6_371_008.8;

/// Builds a [`KIND_VAR_TABLE`] record.
pub fn var_table_record(tick_rate: u32, row_len: u32, num_vars: u32, headers: &[u8]) -> RawRecord {
    let mut payload = Vec::with_capacity(12 + headers.len());
    payload.extend_from_slice(&tick_rate.to_le_bytes());
    payload.extend_from_slice(&row_len.to_le_bytes());
    payload.extend_from_slice(&num_vars.to_le_bytes());
    payload.extend_from_slice(headers);
    RawRecord {
        kind: KIND_VAR_TABLE,
        payload,
    }
}

/// Builds a [`KIND_SESSION_INFO`] record.
pub fn session_info_record(info: &SessionInfo) -> RawRecord {
    RawRecord {
        kind: KIND_SESSION_INFO,
        payload: serde_json::to_vec(info).expect("SessionInfo serializes"),
    }
}

/// Builds a [`KIND_DATA`] record.
pub fn data_record(row: Vec<u8>) -> RawRecord {
    RawRecord {
        kind: KIND_DATA,
        payload: row,
    }
}

/// How a channel value is computed from a row.
#[derive(Debug, Clone)]
enum Extract {
    /// `value * scale + offset`
    Var {
        var: VarHeader,
        scale: f64,
        offset: f64,
    },
    /// Local east coordinate (m) around the start/finish line
    PosX,
    /// Local north coordinate (m) around the start/finish line
    PosY,
}

/// Where the local position comes from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum PosSource {
    /// No position variables
    #[default]
    None,
    /// `Lat`/`Lon` (`.ibt` files)
    Gps,
    /// Integrated velocity and heading (live)
    DeadReckoning,
}

/// A gap in the data longer than this (s) loses the dead-reckoned position.
const DR_MAX_GAP_S: f64 = 1.0;

/// Dead-reckoning state, see the module docs.
#[derive(Debug, Clone, Copy, Default)]
struct DeadReckoning {
    /// Position relative to the start/finish line, once crossed on track.
    pos: Option<(f64, f64)>,
    last_time: Option<f64>,
    lap: Option<i32>,
}

impl DeadReckoning {
    fn reset(&mut self) {
        *self = Self::default();
    }

    /// Advances to `time` with the car's velocity (`vx` forward, `vy` left,
    /// m/s) and heading from north `h` (rad, clockwise).
    fn step(&mut self, time: f64, lap: i32, in_pits: bool, vx: f64, vy: f64, h: f64) {
        if let (Some(t0), Some((x, y))) = (self.last_time, self.pos.as_mut()) {
            let dt = time - t0;
            if !(0.0..=DR_MAX_GAP_S).contains(&dt) || !(vx.is_finite() && vy.is_finite()) {
                self.pos = None;
            } else {
                let (sin, cos) = h.sin_cos();
                *x += (vx * sin - vy * cos) * dt;
                *y += (vx * cos + vy * sin) * dt;
            }
        }
        if self.lap.is_some_and(|l| lap > l) && !in_pits {
            self.pos = Some((0.0, 0.0)); // at the line
        }
        self.last_time = Some(time);
        self.lap = Some(lap);
    }
}

/// Variables the decoder itself needs (not channels).
#[derive(Debug, Clone)]
struct KeyVars {
    session_time: VarHeader,
    session_num: Option<VarHeader>,
    lap: VarHeader,
    lap_completed: Option<VarHeader>,
    lap_dist: VarHeader,
    last_lap_time: Option<VarHeader>,
    on_pit_road: Option<VarHeader>,
    in_pit_stall: Option<VarHeader>,
    track_surface: Option<VarHeader>,
    incidents: Option<VarHeader>,
    is_on_track: Option<VarHeader>,
    tow_time: Option<VarHeader>,
    lat: Option<VarHeader>,
    lon: Option<VarHeader>,
    velocity_x: Option<VarHeader>,
    velocity_y: Option<VarHeader>,
    yaw_north: Option<VarHeader>,
    session_flags: Option<VarHeader>,
}

impl KeyVars {
    fn resolve(t: &VarTable) -> Result<Self, SourceError> {
        let req = |n: &str| {
            t.get(n)
                .cloned()
                .ok_or_else(|| SourceError::InvalidData(format!("required variable {n} missing")))
        };
        let opt = |n: &str| t.get(n).cloned();
        Ok(Self {
            session_time: req("SessionTime")?,
            session_num: opt("SessionNum"),
            lap: req("Lap")?,
            lap_completed: opt("LapCompleted"),
            lap_dist: req("LapDist")?,
            last_lap_time: opt("LapLastLapTime"),
            on_pit_road: opt("OnPitRoad"),
            in_pit_stall: opt("PlayerCarInPitStall"),
            track_surface: opt("PlayerTrackSurface"),
            incidents: opt("PlayerCarMyIncidentCount"),
            is_on_track: opt("IsOnTrack"),
            tow_time: opt("PlayerCarTowTime"),
            lat: opt("Lat"),
            lon: opt("Lon"),
            velocity_x: opt("VelocityX"),
            velocity_y: opt("VelocityY"),
            yaw_north: opt("YawNorth"),
            session_flags: opt("SessionFlags"),
        })
    }
}

/// Values of the previous row, for change detection.
#[derive(Debug, Clone, Copy)]
struct Prev {
    last_lap_time: f64,
    incidents: f64,
    track_surface: f64,
    tow_time: f64,
}

/// `irsdk_Flags` bits (irsdk_defines.h) that make laps not count, with our
/// invalidation reason.
const LAP_VOIDING_FLAGS: [(u32, &str); 3] = [
    (0x0010_0000, "meatball flag (repair required)"), // irsdk_repair
    (0x0001_0000, "black flag"),                      // irsdk_black
    (0x0002_0000, "disqualified"),                    // irsdk_disqualify
];

/// `irsdk_TrkLoc` values (irsdk_defines.h).
const TRK_NOT_IN_WORLD: f64 = -1.0;
const TRK_OFF_TRACK: f64 = 0.0;

/// Decoder for iRacing raw records. See the module docs.
#[derive(Debug, Default)]
pub struct IracingDecoder {
    table: Option<VarTable>,
    keys: Option<KeyVars>,
    tick_rate: f32,
    info: Option<SessionInfo>,
    channels: Vec<(ChannelSpec, Extract)>,
    session_num: Option<i64>,
    meta_dirty: bool,
    session_emitted: bool,
    in_car: bool,
    prev: Option<Prev>,
    pos_source: PosSource,
    dr: DeadReckoning,
}

impl IracingDecoder {
    /// Creates a decoder.
    pub fn new() -> Self {
        Self::default()
    }

    fn on_var_table(&mut self, p: &[u8]) -> Result<(), SourceError> {
        let u = |o: usize| -> Result<usize, SourceError> {
            let b = p
                .get(o..o + 4)
                .ok_or_else(|| SourceError::InvalidData("truncated var table record".into()))?;
            Ok(u32::from_le_bytes(b.try_into().expect("4 bytes")) as usize)
        };
        let (tick_rate, row_len, num_vars) = (u(0)?, u(4)?, u(8)?);
        let headers = p
            .get(12..12 + num_vars * VAR_HEADER_LEN)
            .ok_or_else(|| SourceError::InvalidData("truncated var headers".into()))?;
        let table = VarTable::parse(headers, num_vars, row_len)
            .map_err(|e| SourceError::InvalidData(e.to_string()))?;
        if self.table.as_ref() != Some(&table) {
            self.keys = Some(KeyVars::resolve(&table)?);
            self.table = Some(table);
            self.tick_rate = tick_rate as f32;
            self.prev = None;
            self.meta_dirty = true;
        }
        Ok(())
    }

    fn on_session_info(&mut self, p: &[u8]) -> Result<(), SourceError> {
        let info: SessionInfo = serde_json::from_slice(p)
            .map_err(|e| SourceError::InvalidData(format!("session info: {e}")))?;
        if self.info.as_ref() != Some(&info) {
            self.info = Some(info);
            self.meta_dirty = true;
        }
        Ok(())
    }

    fn build_channels(&mut self) {
        let (Some(t), Some(info)) = (&self.table, &self.info) else {
            return;
        };
        let mut ch = Vec::new();
        let mut var =
            |name: &str, var: &str, unit: Unit, kind: ValueKind, scale: f64, offset: f64| {
                if let Some(v) = t.get(var) {
                    ch.push((
                        ChannelSpec::new(name, unit, kind),
                        Extract::Var {
                            var: v.clone(),
                            scale,
                            offset,
                        },
                    ));
                }
            };
        use ValueKind::{Bool, F32, F64, I32};
        // canonical channels (docs/PLAN.md §4.2)
        var(names::SPEED, "Speed", Unit::MeterPerSecond, F32, 1.0, 0.0);
        var(names::THROTTLE, "Throttle", Unit::Ratio, F32, 1.0, 0.0);
        var(names::BRAKE, "Brake", Unit::Ratio, F32, 1.0, 0.0);
        // iRacing: 0 = disengaged (pedal pressed) … 1 = fully engaged
        var(names::CLUTCH, "Clutch", Unit::Ratio, F32, -1.0, 1.0);
        var(
            names::STEER,
            "SteeringWheelAngle",
            Unit::Radian,
            F32,
            -1.0,
            0.0,
        );
        var(names::GEAR, "Gear", Unit::None, I32, 1.0, 0.0);
        var(names::RPM, "RPM", Unit::Rpm, F32, 1.0, 0.0);
        var(
            names::G_LAT,
            "LatAccel",
            Unit::MeterPerSecondSquared,
            F32,
            -1.0,
            0.0,
        );
        var(
            names::G_LONG,
            "LongAccel",
            Unit::MeterPerSecondSquared,
            F32,
            1.0,
            0.0,
        );
        var(
            names::G_VERT,
            "VertAccel",
            Unit::MeterPerSecondSquared,
            F32,
            1.0,
            0.0,
        );
        var(
            names::YAW_RATE,
            "YawRate",
            Unit::RadianPerSecond,
            F32,
            -1.0,
            0.0,
        );
        var(names::FUEL, "FuelLevel", Unit::Liter, F32, 1.0, 0.0);
        var(
            names::BRAKE_BIAS,
            "dcBrakeBias",
            Unit::Ratio,
            F32,
            0.01,
            0.0,
        );
        var(
            names::ABS_ACTIVE,
            "BrakeABSactive",
            Unit::None,
            Bool,
            1.0,
            0.0,
        );
        // game-specific extras
        var(
            "iracing.lap_dist_pct",
            "LapDistPct",
            Unit::Ratio,
            F32,
            1.0,
            0.0,
        );
        var(
            "iracing.throttle_raw",
            "ThrottleRaw",
            Unit::Ratio,
            F32,
            1.0,
            0.0,
        );
        var("iracing.brake_raw", "BrakeRaw", Unit::Ratio, F32, 1.0, 0.0);
        var(
            "iracing.abs_cut",
            "BrakeABScutPct",
            Unit::Ratio,
            F32,
            1.0,
            0.0,
        );
        var(
            "iracing.tc_setting",
            "dcTractionControl",
            Unit::None,
            F32,
            1.0,
            0.0,
        );
        var("iracing.abs_setting", "dcABS", Unit::None, F32, 1.0, 0.0);
        var(
            "iracing.track_surface",
            "PlayerTrackSurface",
            Unit::None,
            I32,
            1.0,
            0.0,
        );
        var("iracing.yaw", "Yaw", Unit::Radian, F32, 1.0, 0.0);
        var("iracing.yaw_north", "YawNorth", Unit::Radian, F32, 1.0, 0.0);
        var(
            "iracing.velocity_x",
            "VelocityX",
            Unit::MeterPerSecond,
            F32,
            1.0,
            0.0,
        );
        var(
            "iracing.velocity_y",
            "VelocityY",
            Unit::MeterPerSecond,
            F32,
            1.0,
            0.0,
        );
        var(
            "iracing.steering_torque",
            "SteeringWheelTorque",
            Unit::NewtonMeter,
            F32,
            1.0,
            0.0,
        );
        var("iracing.lat", "Lat", Unit::None, F64, 1.0, 0.0);
        var("iracing.lon", "Lon", Unit::None, F64, 1.0, 0.0);
        // local position in metres around the start/finish line
        let has_gps = t.get("Lat").is_some() && t.get("Lon").is_some();
        let has_origin = info.track.latitude_deg.is_some() && info.track.longitude_deg.is_some();
        let has_motion = ["VelocityX", "VelocityY", "YawNorth"]
            .iter()
            .all(|n| t.get(n).is_some());
        self.pos_source = if has_gps && has_origin {
            var(names::POS_Z, "Alt", Unit::Meter, F32, 1.0, 0.0);
            PosSource::Gps
        } else if has_motion {
            PosSource::DeadReckoning
        } else {
            PosSource::None
        };
        if self.pos_source != PosSource::None {
            ch.push((
                ChannelSpec::new(names::POS_X, Unit::Meter, F32),
                Extract::PosX,
            ));
            ch.push((
                ChannelSpec::new(names::POS_Y, Unit::Meter, F32),
                Extract::PosY,
            ));
        }
        self.channels = ch;
    }

    fn meta(&self, session_num: i64) -> Option<SessionMeta> {
        let info = self.info.as_ref()?;
        let sub = info.event.sub_session_id.unwrap_or(0);
        // The driver is part of the key: in an online session every driver
        // has the same subsession, and ids must stay unique when laps are
        // exchanged (.rzx, server).
        let driver = info.player.user_id.unwrap_or(0);
        let session_key = if sub > 0 {
            format!("iracing:{sub}:{session_num}:{driver}")
        } else {
            // offline: identify by track, car and the minute the session started
            format!(
                "iracing:offline:{}:{}:{}:{session_num}:{driver}",
                info.track.id.unwrap_or(0),
                info.player.car_id.unwrap_or(0),
                (info.session_epoch_unix / 60.0).round() as i64
            )
        };
        let def = info.session(session_num);
        let started_at =
            OffsetDateTime::from_unix_timestamp_nanos((info.session_epoch_unix * 1e9) as i128)
                .unwrap_or(OffsetDateTime::UNIX_EPOCH);
        let mut session_info = serde_json::to_value(info).ok();
        if let Some(serde_json::Value::Object(m)) = session_info.as_mut() {
            m.remove("setup"); // stored separately
        }
        Some(SessionMeta {
            session_key,
            game: GameId::Iracing,
            game_version: info.event.build_version.clone(),
            track_key: format!("iracing:{}", info.track.id.unwrap_or(0)),
            track_name: track_name(info),
            car_key: format!("iracing:{}", info.player.car_id.unwrap_or(0)),
            car_name: info
                .player
                .car_screen_name
                .clone()
                .or_else(|| info.player.car_path.clone())
                .unwrap_or_else(|| "Unknown car".into()),
            session_type: session_type(def.and_then(|d| d.session_type.as_deref())),
            started_at,
            conditions: Conditions {
                air_temp_c: info.weather.air_temp_c.map(|v| v as f32),
                track_temp_c: info.weather.track_temp_c.map(|v| v as f32),
                weather: info.weather.skies.clone(),
            },
            setup: info.setup.clone(),
            session_info,
            sample_rate_hz: if self.tick_rate > 0.0 {
                self.tick_rate
            } else {
                60.0
            },
            channels: self.channels.iter().map(|(s, _)| s.clone()).collect(),
        })
    }

    fn on_data(&mut self, row: &[u8], out: &mut Vec<SourceEvent>) {
        let Some(k) = self.keys.clone() else {
            return; // data before the var table: cannot interpret
        };
        let get = |v: &VarHeader| v.read(row, 0);
        let get_opt = |v: &Option<VarHeader>| v.as_ref().and_then(|v| v.read(row, 0));

        let on_track = get_opt(&k.is_on_track).is_none_or(|v| v != 0.0);
        let surface = get_opt(&k.track_surface).unwrap_or(3.0);
        if !on_track || surface == TRK_NOT_IN_WORLD {
            if self.in_car {
                out.push(SourceEvent::LapAborted {
                    reason: "left the car".into(),
                });
            }
            self.in_car = false;
            self.prev = None;
            self.dr.reset();
            return;
        }
        self.in_car = true;

        let session_num = get_opt(&k.session_num).unwrap_or(0.0) as i64;
        if self.meta_dirty || self.session_num != Some(session_num) {
            if self.session_num != Some(session_num) {
                self.dr.reset();
            }
            self.build_channels();
            if let Some(meta) = self.meta(session_num) {
                out.push(SourceEvent::Session(Box::new(meta)));
                self.session_emitted = true;
                self.meta_dirty = false;
                self.session_num = Some(session_num);
            }
        }
        if !self.session_emitted {
            return; // no session info yet
        }

        let tow = get_opt(&k.tow_time).unwrap_or(0.0);
        let incidents = get_opt(&k.incidents).unwrap_or(0.0);
        let last_lap_time = get_opt(&k.last_lap_time).unwrap_or(0.0);
        let lap_completed = get_opt(&k.lap_completed).unwrap_or(0.0) as i32;
        let prev = self.prev;
        self.prev = Some(Prev {
            last_lap_time,
            incidents,
            track_surface: surface,
            tow_time: tow,
        });

        if tow > 0.0 {
            if prev.is_some_and(|p| p.tow_time <= 0.0) {
                out.push(SourceEvent::LapAborted {
                    reason: "towed".into(),
                });
            }
            self.dr.reset();
            return; // the car is being moved: not driving data
        }

        let (Some(session_time), Some(lap), Some(lap_dist)) =
            (get(&k.session_time), get(&k.lap), get(&k.lap_dist))
        else {
            return;
        };
        let in_pits = get_opt(&k.on_pit_road).is_some_and(|v| v != 0.0)
            || get_opt(&k.in_pit_stall).is_some_and(|v| v != 0.0);
        let flags = get_opt(&k.session_flags).unwrap_or(0.0) as u32;
        let pos = match self.pos_source {
            PosSource::None => None,
            PosSource::Gps => {
                let origin = self
                    .info
                    .as_ref()
                    .and_then(|i| Some((i.track.latitude_deg?, i.track.longitude_deg?)));
                match (get_opt(&k.lat), get_opt(&k.lon), origin) {
                    (Some(lat), Some(lon), Some((lat0, lon0))) => Some((
                        (lon - lon0).to_radians() * EARTH_RADIUS_M * lat0.to_radians().cos(),
                        (lat - lat0).to_radians() * EARTH_RADIUS_M,
                    )),
                    _ => None,
                }
            }
            PosSource::DeadReckoning => {
                let v = |h: &Option<VarHeader>| get_opt(h).unwrap_or(f64::NAN);
                self.dr.step(
                    session_time,
                    lap as i32,
                    in_pits,
                    v(&k.velocity_x),
                    v(&k.velocity_y),
                    v(&k.yaw_north),
                );
                self.dr.pos
            }
        };
        let values = self
            .channels
            .iter()
            .map(|(_, e)| match e {
                Extract::Var { var, scale, offset } => {
                    var.read(row, 0).map_or(f64::NAN, |v| v * scale + offset)
                }
                Extract::PosX => pos.map_or(f64::NAN, |p| p.0),
                Extract::PosY => pos.map_or(f64::NAN, |p| p.1),
            })
            .collect();
        out.push(SourceEvent::Frame(Frame {
            session_time,
            lap: lap as i32,
            lap_dist: lap_dist as f32,
            in_pits,
            values,
        }));
        if !in_pits {
            for (bit, reason) in LAP_VOIDING_FLAGS {
                if flags & bit != 0 {
                    // repeated every row; the recorder keeps each reason once
                    out.push(SourceEvent::Invalidate {
                        lap: None,
                        reason: reason.into(),
                    });
                }
            }
        }

        let Some(prev) = prev else {
            return; // no change detection on the first row
        };
        if incidents > prev.incidents {
            out.push(SourceEvent::Invalidate {
                lap: None,
                reason: format!("incident (+{}x)", (incidents - prev.incidents) as i64),
            });
        }
        if surface == TRK_OFF_TRACK && prev.track_surface != TRK_OFF_TRACK {
            out.push(SourceEvent::Invalidate {
                lap: None,
                reason: "off track".into(),
            });
        }
        if last_lap_time != prev.last_lap_time {
            if last_lap_time > 0.0 {
                out.push(SourceEvent::LapTime {
                    lap: lap_completed,
                    lap_time_ms: (last_lap_time * 1000.0).round() as u32,
                    sector_times_ms: Vec::new(),
                });
            } else if last_lap_time < 0.0 {
                out.push(SourceEvent::Invalidate {
                    lap: Some(lap_completed),
                    reason: "invalidated by iRacing".into(),
                });
            }
        }
    }
}

impl Decoder for IracingDecoder {
    fn decode(&mut self, record: &RawRecord) -> Result<Vec<SourceEvent>, SourceError> {
        let mut out = Vec::new();
        match record.kind {
            KIND_VAR_TABLE => self.on_var_table(&record.payload)?,
            KIND_SESSION_INFO => self.on_session_info(&record.payload)?,
            KIND_DATA => self.on_data(&record.payload, &mut out),
            other => tracing::warn!(kind = other, "unknown iRacing record kind ignored"),
        }
        Ok(out)
    }

    fn finish(&mut self) -> Vec<SourceEvent> {
        if std::mem::take(&mut self.session_emitted) {
            vec![SourceEvent::SessionEnd]
        } else {
            Vec::new()
        }
    }
}

/// "Display name – Config", e.g. "Motorsport Arena Oschersleben – Grand Prix".
fn track_name(info: &SessionInfo) -> String {
    let t = &info.track;
    let base = t
        .display_name
        .clone()
        .or_else(|| t.name.clone())
        .unwrap_or_else(|| "Unknown track".into());
    match t.config_name.as_deref().map(str::trim) {
        Some(cfg) if !cfg.is_empty() && !base.contains(cfg) => format!("{base} – {cfg}"),
        _ => base,
    }
}

/// Maps iRacing's `SessionType` text to our session type.
fn session_type(s: Option<&str>) -> SessionType {
    let s = s.unwrap_or_default().to_ascii_lowercase();
    if s.contains("qualify") {
        SessionType::Qualifying
    } else if s.contains("race") {
        SessionType::Race
    } else if s.contains("time trial") {
        SessionType::TimeTrial
    } else if s.contains("practice") || s.contains("warmup") || s.contains("testing") {
        SessionType::Practice
    } else {
        SessionType::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::VarType;
    use crate::format::testutil::{build_var_table, put};
    use crate::session_info::{PlayerInfo, TrackInfo};

    struct Rig {
        dec: IracingDecoder,
        table: VarTable,
        row_len: usize,
    }

    impl Rig {
        fn new() -> Self {
            let defs = [
                ("SessionTime", VarType::Double, 1, "s"),
                ("SessionNum", VarType::Int, 1, ""),
                ("IsOnTrack", VarType::Bool, 1, ""),
                ("PlayerTrackSurface", VarType::Int, 1, "irsdk_TrkLoc"),
                ("PlayerCarMyIncidentCount", VarType::Int, 1, ""),
                ("PlayerCarTowTime", VarType::Float, 1, "s"),
                ("OnPitRoad", VarType::Bool, 1, ""),
                ("SessionFlags", VarType::BitField, 1, "irsdk_Flags"),
                ("Lap", VarType::Int, 1, ""),
                ("LapCompleted", VarType::Int, 1, ""),
                ("LapDist", VarType::Float, 1, "m"),
                ("LapLastLapTime", VarType::Float, 1, "s"),
                ("Speed", VarType::Float, 1, "m/s"),
                ("Clutch", VarType::Float, 1, "%"),
                ("SteeringWheelAngle", VarType::Float, 1, "rad"),
                ("dcBrakeBias", VarType::Float, 1, ""),
                ("Lat", VarType::Double, 1, "deg"),
                ("Lon", VarType::Double, 1, "deg"),
            ];
            let (headers, row_len) = build_var_table(&defs);
            let table = VarTable::parse(&headers, defs.len(), row_len).unwrap();
            let mut dec = IracingDecoder::new();
            dec.decode(&var_table_record(
                60,
                row_len as u32,
                defs.len() as u32,
                &headers,
            ))
            .unwrap();
            let info = SessionInfo {
                session_epoch_unix: 1_794_230_824.0,
                track: TrackInfo {
                    id: Some(449),
                    latitude_deg: Some(52.0),
                    longitude_deg: Some(11.0),
                    ..Default::default()
                },
                event: crate::session_info::EventInfo {
                    sub_session_id: Some(77),
                    ..Default::default()
                },
                sessions: vec![crate::session_info::SessionDef {
                    num: 2,
                    session_type: Some("Race".into()),
                    name: None,
                }],
                player: PlayerInfo {
                    car_id: Some(163),
                    user_id: Some(4242),
                    ..Default::default()
                },
                ..Default::default()
            };
            dec.decode(&session_info_record(&info)).unwrap();
            Self {
                dec,
                table,
                row_len,
            }
        }

        fn row(&mut self, set: &[(&str, f64)]) -> Vec<SourceEvent> {
            let mut row = vec![0u8; self.row_len];
            let defaults = [
                ("IsOnTrack", 1.0),
                ("PlayerTrackSurface", 3.0),
                ("SessionNum", 2.0),
                ("Lap", 1.0),
                ("Clutch", 1.0),
                ("dcBrakeBias", 56.0),
                ("Lat", 52.0),
                ("Lon", 11.0),
            ];
            for (name, v) in defaults.iter().chain(set) {
                put(&mut row, self.table.get(name).unwrap(), 0, *v);
            }
            self.dec.decode(&data_record(row)).unwrap()
        }
    }

    fn frame(events: &[SourceEvent]) -> &Frame {
        events
            .iter()
            .find_map(|e| match e {
                SourceEvent::Frame(f) => Some(f),
                _ => None,
            })
            .expect("frame")
    }

    #[test]
    fn first_row_starts_session_with_channels() {
        let mut rig = Rig::new();
        let ev = rig.row(&[("SessionTime", 10.0), ("Speed", 40.0)]);
        let SourceEvent::Session(meta) = &ev[0] else {
            panic!("session first: {ev:?}")
        };
        assert_eq!(meta.session_key, "iracing:77:2:4242");
        assert_eq!(meta.track_key, "iracing:449");
        assert_eq!(meta.car_key, "iracing:163");
        assert_eq!(meta.session_type, SessionType::Race);
        assert_eq!(meta.sample_rate_hz, 60.0);
        let names: Vec<_> = meta.channels.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&names::SPEED) && names.contains(&names::POS_X));
        let f = frame(&ev);
        assert_eq!(f.session_time, 10.0);
        assert_eq!(f.lap, 1);
    }

    #[test]
    fn conversions_follow_canonical_conventions() {
        let mut rig = Rig::new();
        let ev = rig.row(&[
            ("SteeringWheelAngle", 0.5), // iRacing: left
            ("Clutch", 0.25),            // mostly disengaged = pedal mostly pressed
            ("Lat", 52.001),
            ("Lon", 11.001),
        ]);
        let SourceEvent::Session(meta) = &ev[0] else {
            panic!()
        };
        let value = |name: &str| {
            let i = meta.channels.iter().position(|c| c.name == name).unwrap();
            frame(&ev).values[i]
        };
        assert!((value(names::STEER) + 0.5).abs() < 1e-6, "positive = right");
        assert!((value(names::CLUTCH) - 0.75).abs() < 1e-6, "pedal position");
        assert!((value(names::BRAKE_BIAS) - 0.56).abs() < 1e-6);
        assert!((value(names::POS_Y) - 111.195).abs() < 0.01, "0.001° north");
        assert!(
            (value(names::POS_X) - 68.459).abs() < 0.01,
            "0.001° east at 52°N"
        );
    }

    #[test]
    fn detects_lap_time_invalidations_and_incidents() {
        let mut rig = Rig::new();
        rig.row(&[
            ("Lap", 3.0),
            ("LapCompleted", 2.0),
            ("LapLastLapTime", -1.0),
        ]);
        // unchanged -1: no information
        let ev = rig.row(&[
            ("Lap", 3.0),
            ("LapCompleted", 2.0),
            ("LapLastLapTime", -1.0),
        ]);
        assert_eq!(ev.len(), 1, "only the frame: {ev:?}");
        let ev = rig.row(&[
            ("Lap", 3.0),
            ("LapCompleted", 2.0),
            ("LapLastLapTime", 93.586),
        ]);
        assert!(ev.contains(&SourceEvent::LapTime {
            lap: 2,
            lap_time_ms: 93_586,
            sector_times_ms: vec![]
        }));
        let ev = rig.row(&[
            ("Lap", 4.0),
            ("LapCompleted", 3.0),
            ("LapLastLapTime", -1.0),
            ("PlayerCarMyIncidentCount", 2.0),
            ("PlayerTrackSurface", 0.0),
        ]);
        assert!(ev.contains(&SourceEvent::Invalidate {
            lap: Some(3),
            reason: "invalidated by iRacing".into()
        }));
        assert!(ev.contains(&SourceEvent::Invalidate {
            lap: None,
            reason: "incident (+2x)".into()
        }));
        assert!(ev.contains(&SourceEvent::Invalidate {
            lap: None,
            reason: "off track".into()
        }));
    }

    #[test]
    fn meatball_flag_voids_laps_on_track_only() {
        let mut rig = Rig::new();
        let meatball = f64::from(0x0010_0000 | 0x0004_0000); // repair + servicible
        let ev = rig.row(&[("SessionFlags", meatball)]);
        assert!(ev.contains(&SourceEvent::Invalidate {
            lap: None,
            reason: "meatball flag (repair required)".into()
        }));
        let ev = rig.row(&[("SessionFlags", meatball), ("OnPitRoad", 1.0)]);
        assert!(
            !ev.iter()
                .any(|e| matches!(e, SourceEvent::Invalidate { .. })),
            "in the pit lane the repair is being done: {ev:?}"
        );
        let ev = rig.row(&[("SessionFlags", f64::from(0x0004_0000))]);
        assert!(
            !ev.iter()
                .any(|e| matches!(e, SourceEvent::Invalidate { .. }))
        );
    }

    #[test]
    fn skips_rows_out_of_the_car_and_reports_tows() {
        let mut rig = Rig::new();
        let ev = rig.row(&[("PlayerTrackSurface", -1.0)]);
        assert!(ev.is_empty(), "stale first row skipped: {ev:?}");
        rig.row(&[("SessionTime", 1.0)]);
        let ev = rig.row(&[("SessionTime", 2.0), ("PlayerCarTowTime", 30.0)]);
        assert_eq!(
            ev,
            vec![SourceEvent::LapAborted {
                reason: "towed".into()
            }]
        );
        let ev = rig.row(&[("IsOnTrack", 0.0)]);
        assert_eq!(
            ev,
            vec![SourceEvent::LapAborted {
                reason: "left the car".into()
            }]
        );
        assert!(rig.dec.finish().contains(&SourceEvent::SessionEnd));
    }

    #[test]
    fn session_type_mapping() {
        assert_eq!(session_type(Some("Lone Qualify")), SessionType::Qualifying);
        assert_eq!(session_type(Some("Open Qualify")), SessionType::Qualifying);
        assert_eq!(session_type(Some("Race")), SessionType::Race);
        assert_eq!(session_type(Some("Offline Testing")), SessionType::Practice);
        assert_eq!(session_type(Some("Practice")), SessionType::Practice);
        assert_eq!(session_type(None), SessionType::Other);
    }
}
