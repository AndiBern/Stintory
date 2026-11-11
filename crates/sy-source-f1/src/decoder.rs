// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Turns F1 25 packets into [`SourceEvent`]s.
//!
//! # Raw records
//!
//! | kind | payload |
//! |---|---|
//! | [`KIND_PACKET`] | receive time (Unix ms, `i64` LE) + one UDP packet, reduced to the player's car ([`crate::filter`]) |
//!
//! The receive time makes the session's wall-clock start reproducible when a
//! spool file is replayed (the packets have no wall clock).
//!
//! # Behaviour
//!
//! - A session is one `sessionUID` of the player's car index (in online
//!   sessions every driver has another index, so session and lap ids stay
//!   unique per driver). A new `sessionUID` ends the previous session, as
//!   does the event `SEND`.
//! - One frame per car telemetry packet (sent at the in-game rate), with
//!   the latest motion, lap data, status, extra motion and (2026) telemetry 2
//!   of the player's car.
//! - No frames in the garage (`driverStatus` 0), while spectating (that's
//!   another driver's car) or before the session info is known. Going back
//!   to the garage mid-lap aborts the lap.
//! - Lap times: when the lap number goes up, `lastLapTimeInMS` is the
//!   official time of the lap that ended; sectors 1 and 2 are the last ones
//!   seen in that lap, sector 3 the rest.
//! - Validity: `currentLapInvalid` as it was at the end of the lap (a
//!   flashback can undo an invalidation, so the state while driving is not
//!   final), and the lap-valid bit of the player's session history.
//! - Flashback (`FLBK`): rewinds to the event's session time; the recorder
//!   drops the rewound samples and flags the lap.
//! - Track key `f1_25:<trackId>`, car key `f1_25:<teamId>/<formula>`.

use sy_model::capture::{
    ChannelSpec, Decoder, Frame, RawRecord, SessionMeta, SourceError, SourceEvent, ValueKind,
};
use sy_model::channel::names;
use sy_model::session::Conditions;
use sy_model::{GameId, Unit};
use time::OffsetDateTime;

use crate::packets::{
    self, CarMotion, CarSetup, CarStatus, CarTelemetry, CarTelemetry2, Event, Format, Header,
    LapData, MotionEx, Participant, SessionHistory, id,
};
use crate::tables;

/// Raw record: receive time + one packet.
pub const KIND_PACKET: u8 = 1;

/// Seconds of session time to wait for the participants packet (team id)
/// before starting without it. The game sends it every 5 s.
const PARTICIPANTS_WAIT_S: f32 = 6.0;

/// 1 PSI in kPa.
const PSI_KPA: f64 = 6.894_757;
/// Standard gravity, m/s².
const G: f64 = 9.806_65;

/// Builds a [`KIND_PACKET`] record.
pub fn packet_record(received_unix_ms: i64, packet: &[u8]) -> RawRecord {
    let mut payload = Vec::with_capacity(8 + packet.len());
    payload.extend_from_slice(&received_unix_ms.to_le_bytes());
    payload.extend_from_slice(packet);
    RawRecord {
        kind: KIND_PACKET,
        payload,
    }
}

/// Decoder for F1 25 packets (both formats).
#[derive(Debug, Default)]
pub struct F1Decoder {
    s: Option<SessionState>,
}

#[derive(Debug)]
struct SessionState {
    uid: u64,
    player: u8,
    format: Format,
    game_version: String,
    started_at: OffsetDateTime,
    first_time: f32,
    session: Option<packets::Session>,
    participant: Option<Participant>,
    setup: Option<CarSetup>,
    motion: Option<CarMotion>,
    lap: Option<LapData>,
    status: Option<CarStatus>,
    motion_ex: Option<MotionEx>,
    telemetry2: Option<CarTelemetry2>,
    /// Session times of the first telemetry packets (sample rate)
    telemetry_times: Vec<f32>,
    meta: Option<SessionMeta>,
    /// After `SEND`, until `SSTA`
    ended: bool,
    last_frame_time: Option<f64>,
    /// Lap number of the last frame
    frame_lap: Option<i32>,
    /// Laps of the session history already reported invalid
    history_invalid: Vec<u8>,
}

/// One channel and how to compute it.
struct Ch {
    spec: ChannelSpec,
    get: fn(&Inputs) -> f64,
}

/// Inputs of one frame.
struct Inputs<'a> {
    t: &'a CarTelemetry,
    m: Option<&'a CarMotion>,
    s: Option<&'a CarStatus>,
    x: Option<&'a MotionEx>,
    t2: Option<&'a CarTelemetry2>,
}

fn opt<T>(v: Option<&T>, f: impl Fn(&T) -> f64) -> f64 {
    v.map_or(f64::NAN, f)
}

fn ch(name: impl Into<String>, unit: Unit, kind: ValueKind, get: fn(&Inputs) -> f64) -> Ch {
    Ch {
        spec: ChannelSpec::new(name, unit, kind),
        get,
    }
}

/// The channels of a format, in frame order.
fn channels(format: Format) -> Vec<Ch> {
    use ValueKind::{Bool, F32, I32};
    let mut v = vec![
        ch(names::SPEED, Unit::MeterPerSecond, F32, |i| {
            f64::from(i.t.speed_kmh) / 3.6
        }),
        ch(names::THROTTLE, Unit::Ratio, F32, |i| {
            f64::from(i.t.throttle)
        }),
        ch(names::BRAKE, Unit::Ratio, F32, |i| f64::from(i.t.brake)),
        ch(names::CLUTCH, Unit::Ratio, F32, |i| {
            f64::from(i.t.clutch) / 100.0
        }),
        ch(names::STEER_NORM, Unit::Ratio, F32, |i| {
            f64::from(i.t.steer)
        }),
        ch(names::GEAR, Unit::None, I32, |i| f64::from(i.t.gear)),
        ch(names::RPM, Unit::Rpm, F32, |i| f64::from(i.t.rpm)),
        // world X/Z are horizontal, Y is up
        ch(names::POS_X, Unit::Meter, F32, |i| {
            opt(i.m, |m| f64::from(m.position[0]))
        }),
        ch(names::POS_Y, Unit::Meter, F32, |i| {
            opt(i.m, |m| f64::from(m.position[2]))
        }),
        ch(names::POS_Z, Unit::Meter, F32, |i| {
            opt(i.m, |m| f64::from(m.position[1]))
        }),
        ch(names::G_LAT, Unit::MeterPerSecondSquared, F32, |i| {
            opt(i.m, |m| f64::from(m.g_force[0]) * G)
        }),
        ch(names::G_LONG, Unit::MeterPerSecondSquared, F32, |i| {
            opt(i.m, |m| f64::from(m.g_force[1]) * G)
        }),
        ch(names::G_VERT, Unit::MeterPerSecondSquared, F32, |i| {
            opt(i.m, |m| f64::from(m.g_force[2]) * G)
        }),
        ch(names::YAW, Unit::Radian, F32, |i| {
            opt(i.m, |m| f64::from(m.yaw_pitch_roll[0]))
        }),
        ch(names::FUEL, Unit::Kilogram, F32, |i| {
            opt(i.s, |s| f64::from(s.fuel_kg))
        }),
        ch(names::BRAKE_BIAS, Unit::Ratio, F32, |i| {
            opt(i.s, |s| f64::from(s.front_brake_bias) / 100.0)
        }),
        ch("f1.drs", Unit::None, Bool, |i| f64::from(u8::from(i.t.drs))),
        ch("f1.ers_store", Unit::Joule, F32, |i| {
            opt(i.s, |s| f64::from(s.ers_store_j))
        }),
        ch("f1.ers_deploy_mode", Unit::None, I32, |i| {
            opt(i.s, |s| f64::from(s.ers_deploy_mode))
        }),
        ch("f1.ers_deployed_lap", Unit::Joule, F32, |i| {
            opt(i.s, |s| f64::from(s.ers_deployed_lap_j))
        }),
        ch("f1.engine_temp", Unit::Celsius, F32, |i| {
            f64::from(i.t.engine_temp_c)
        }),
        ch("f1.front_wheels_angle", Unit::Radian, F32, |i| {
            opt(i.x, |x| f64::from(x.front_wheels_angle))
        }),
    ];
    // per wheel; closures can't capture the index, so one entry per wheel
    macro_rules! wheels {
        ($prefix:literal, $unit:expr, |$i:ident, $w:ident| $e:expr) => {
            v.push(ch(concat!($prefix, "_rl"), $unit, F32, |$i| {
                let $w = 0;
                $e
            }));
            v.push(ch(concat!($prefix, "_rr"), $unit, F32, |$i| {
                let $w = 1;
                $e
            }));
            v.push(ch(concat!($prefix, "_fl"), $unit, F32, |$i| {
                let $w = 2;
                $e
            }));
            v.push(ch(concat!($prefix, "_fr"), $unit, F32, |$i| {
                let $w = 3;
                $e
            }));
        };
    }
    wheels!("f1.tyre_surface_temp", Unit::Celsius, |i, w| f64::from(
        i.t.tyre_surface_temp_c[w]
    ));
    wheels!("f1.tyre_inner_temp", Unit::Celsius, |i, w| f64::from(
        i.t.tyre_inner_temp_c[w]
    ));
    wheels!("f1.brake_temp", Unit::Celsius, |i, w| f64::from(
        i.t.brake_temp_c[w]
    ));
    wheels!("f1.tyre_pressure", Unit::Kilopascal, |i, w| f64::from(
        i.t.tyre_pressure_psi[w]
    ) * PSI_KPA);
    wheels!("f1.wheel_slip_ratio", Unit::Ratio, |i, w| opt(i.x, |x| {
        f64::from(x.wheel_slip_ratio[w])
    }));
    wheels!("f1.suspension_position", Unit::None, |i, w| opt(i.x, |x| {
        f64::from(x.suspension_position[w])
    }));
    if format == Format::F2026 {
        v.push(ch("f1.active_aero_mode", Unit::None, I32, |i| {
            opt(i.t2, |t| f64::from(t.active_aero_mode))
        }));
        v.push(ch("f1.overtake_active", Unit::None, Bool, |i| {
            opt(i.t2, |t| f64::from(u8::from(t.overtake_active)))
        }));
    }
    v
}

/// Nominal sample rate from the spacing of the first telemetry packets,
/// rounded to the rates the game offers.
fn sample_rate(times: &[f32]) -> f32 {
    let mut gaps: Vec<f32> = times
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|g| *g > 0.0)
        .collect();
    if gaps.is_empty() {
        return 60.0;
    }
    gaps.sort_by(f32::total_cmp);
    let hz = 1.0 / gaps[gaps.len() / 2];
    [10.0f32, 15.0, 20.0, 30.0, 60.0]
        .into_iter()
        .min_by(|a, b| (a - hz).abs().total_cmp(&(b - hz).abs()))
        .unwrap_or(60.0)
}

impl SessionState {
    fn new(h: &Header, received_ms: i64) -> Self {
        let wall = OffsetDateTime::from_unix_timestamp_nanos(i128::from(received_ms) * 1_000_000)
            .unwrap_or(OffsetDateTime::UNIX_EPOCH);
        Self {
            uid: h.session_uid,
            player: h.player_car,
            format: h.format,
            game_version: format!("{}.{:02}", h.game_major, h.game_minor),
            started_at: wall - time::Duration::seconds_f64(f64::from(h.session_time).max(0.0)),
            first_time: h.session_time,
            session: None,
            participant: None,
            setup: None,
            motion: None,
            lap: None,
            status: None,
            motion_ex: None,
            telemetry2: None,
            telemetry_times: Vec::new(),
            meta: None,
            ended: false,
            last_frame_time: None,
            frame_lap: None,
            history_invalid: Vec::new(),
        }
    }

    fn key(&self) -> String {
        format!("f1_25:{:016x}:{}", self.uid, self.player)
    }

    /// Session info, once enough is known.
    fn build_meta(&self, now: f32) -> Option<SessionMeta> {
        let s = self.session.as_ref()?;
        if self.participant.is_none() && now - self.first_time < PARTICIPANTS_WAIT_S {
            return None;
        }
        let team = self.participant.as_ref().map(|p| p.team_id);
        let car_key = match team {
            Some(t) => format!("f1_25:{t}/{}", s.formula),
            None => format!("f1_25:unknown/{}", s.formula),
        };
        let p = self.participant.as_ref();
        let info = serde_json::json!({
            "udp_format": self.format.packet_format(),
            "session_type": tables::session_type_name(s.session_type),
            "session_type_id": s.session_type,
            "formula": tables::formula(s.formula),
            "track_id": s.track_id,
            "track_length_m": s.track_length_m,
            "total_laps": s.total_laps,
            "network_game": s.network_game,
            "ai_difficulty": s.ai_difficulty,
            "game_mode": s.game_mode,
            "rule_set": s.rule_set,
            "recovery_mode": s.recovery_mode,
            "sector2_start_m": s.sector2_start_m,
            "sector3_start_m": s.sector3_start_m,
            "assists": {
                "steering": s.assists[0], "braking": s.assists[1], "gearbox": s.assists[2],
                "pit": s.assists[3], "pit_release": s.assists[4], "ers": s.assists[5],
                "drs": s.assists[6], "racing_line": s.assists[7], "racing_line_type": s.assists[8],
            },
            "team_id": team,
            "race_number": p.map(|p| p.race_number),
            "telemetry_public": p.map(|p| p.telemetry_public),
        });
        Some(SessionMeta {
            session_key: self.key(),
            game: GameId::F1_25,
            game_version: Some(self.game_version.clone()),
            track_key: format!("f1_25:{}", s.track_id),
            track_name: tables::track_name(s.track_id),
            car_key,
            car_name: tables::car_name(s.formula, team),
            session_type: tables::session_type(s.session_type),
            started_at: self.started_at,
            conditions: Conditions {
                air_temp_c: Some(f32::from(s.air_temp_c)),
                track_temp_c: Some(f32::from(s.track_temp_c)),
                weather: tables::weather(s.weather).map(str::to_owned),
            },
            setup: self.setup.as_ref().map(CarSetup::to_json),
            session_info: Some(info),
            sample_rate_hz: sample_rate(&self.telemetry_times),
            channels: channels(self.format).into_iter().map(|c| c.spec).collect(),
        })
    }

    /// Emits session info if it changed (or for the first time).
    fn update_meta(&mut self, now: f32, out: &mut Vec<SourceEvent>) {
        let Some(mut meta) = self.build_meta(now) else {
            return;
        };
        if let Some(old) = &self.meta {
            // the rate is measured once
            meta.sample_rate_hz = old.sample_rate_hz;
            if *old == meta {
                return;
            }
        } else if self.telemetry_times.len() < 2 {
            return;
        }
        self.meta = Some(meta.clone());
        out.push(SourceEvent::Session(Box::new(meta)));
    }

    fn on_lap_data(&mut self, ld: LapData, out: &mut Vec<SourceEvent>) {
        let prev = self.lap.replace(ld);
        if self.meta.is_none() || self.ended {
            return;
        }
        let Some(prev) = prev else { return };
        if ld.lap > prev.lap {
            let lap = i32::from(prev.lap);
            if ld.last_lap_ms > 0 {
                let (s1, s2) = (prev.sector1_ms, prev.sector2_ms);
                let sectors = if s1 > 0 && s2 > 0 && s1 + s2 < ld.last_lap_ms {
                    vec![s1, s2, ld.last_lap_ms - s1 - s2]
                } else {
                    Vec::new()
                };
                out.push(SourceEvent::LapTime {
                    lap,
                    lap_time_ms: ld.last_lap_ms,
                    sector_times_ms: sectors,
                });
            }
            if prev.lap_invalid {
                out.push(SourceEvent::Invalidate {
                    lap: Some(lap),
                    reason: "invalidated by the game".into(),
                });
            }
        }
        if ld.driver_status == 0 && prev.driver_status != 0 && self.frame_lap.is_some() {
            out.push(SourceEvent::LapAborted {
                reason: "returned to the garage".into(),
            });
            self.frame_lap = None;
        }
    }

    fn on_history(&mut self, history: &SessionHistory, out: &mut Vec<SourceEvent>) {
        if self.meta.is_none() || history.car != self.player {
            return;
        }
        let current = self.lap.map_or(0, |l| l.lap);
        for (i, lap) in history.laps.iter().enumerate() {
            let Ok(number) = u8::try_from(i + 1) else {
                break;
            };
            // completed laps only; the last entry is the lap being driven
            if number >= current || lap.lap_time_ms == 0 || lap.lap_valid() {
                continue;
            }
            if !self.history_invalid.contains(&number) {
                self.history_invalid.push(number);
                out.push(SourceEvent::Invalidate {
                    lap: Some(i32::from(number)),
                    reason: "invalidated by the game".into(),
                });
            }
        }
    }

    fn on_telemetry(&mut self, h: &Header, t: CarTelemetry, out: &mut Vec<SourceEvent>) {
        if self.telemetry_times.len() < 32 {
            self.telemetry_times.push(h.session_time);
        }
        self.update_meta(h.session_time, out);
        if self.meta.is_none() || self.ended {
            return;
        }
        let Some(ld) = self.lap else { return };
        if self.session.as_ref().is_some_and(|s| s.spectating) || ld.driver_status == 0 {
            return;
        }
        let time = f64::from(h.session_time);
        if self.last_frame_time.is_some_and(|last| time <= last) {
            return; // paused or repeated
        }
        self.last_frame_time = Some(time);
        let inputs = Inputs {
            t: &t,
            m: self.motion.as_ref(),
            s: self.status.as_ref(),
            x: self.motion_ex.as_ref(),
            t2: self.telemetry2.as_ref(),
        };
        let values = channels(self.format)
            .iter()
            .map(|c| (c.get)(&inputs))
            .collect();
        let lap = i32::from(ld.lap);
        self.frame_lap = Some(lap);
        out.push(SourceEvent::Frame(Frame {
            session_time: time,
            lap,
            lap_dist: ld.lap_distance_m,
            in_pits: ld.pit_status != 0,
            values,
        }));
    }

    fn on_event(&mut self, event: Event, out: &mut Vec<SourceEvent>) {
        match event {
            Event::Flashback { session_time, .. } if self.meta.is_some() => {
                let to = f64::from(session_time);
                out.push(SourceEvent::Rewind {
                    to_session_time: to,
                });
                self.last_frame_time = Some(to - 1e-6);
            }
            Event::SessionEnded => {
                if self.meta.is_some() && !self.ended {
                    out.push(SourceEvent::SessionEnd);
                }
                self.ended = true;
            }
            Event::SessionStarted => self.ended = false,
            _ => {}
        }
    }
}

impl F1Decoder {
    /// Creates a decoder.
    pub fn new() -> Self {
        Self::default()
    }

    fn on_packet(&mut self, received_ms: i64, packet: &[u8]) -> Vec<SourceEvent> {
        let mut out = Vec::new();
        let h = match Header::parse(packet) {
            Ok(h) => h,
            Err(e) => {
                tracing::debug!(error = %e, "packet ignored");
                return out;
            }
        };
        let same = self
            .s
            .as_ref()
            .is_some_and(|s| s.uid == h.session_uid && s.player == h.player_car);
        if !same {
            if let Some(old) = self.s.take()
                && old.meta.is_some()
                && !old.ended
            {
                out.push(SourceEvent::SessionEnd);
            }
            self.s = Some(SessionState::new(&h, received_ms));
        }
        let s = self.s.as_mut().expect("set above");
        if s.format != h.format {
            tracing::warn!("UDP format changed within a session, packet ignored");
            return out;
        }
        match h.packet_id {
            id::SESSION => {
                s.session = Some(packets::Session::parse(&h, packet));
                s.update_meta(h.session_time, &mut out);
            }
            id::PARTICIPANTS => {
                s.participant = Some(Participant::parse(&h, packet));
                s.update_meta(h.session_time, &mut out);
            }
            id::CAR_SETUPS => {
                s.setup = Some(CarSetup::parse(&h, packet));
                if s.meta.is_some() {
                    s.update_meta(h.session_time, &mut out);
                }
            }
            id::LAP_DATA => s.on_lap_data(LapData::parse(&h, packet), &mut out),
            id::MOTION => s.motion = Some(CarMotion::parse(&h, packet)),
            id::CAR_STATUS => s.status = Some(CarStatus::parse(&h, packet)),
            id::MOTION_EX => s.motion_ex = Some(MotionEx::parse(packet)),
            id::CAR_TELEMETRY_2 => s.telemetry2 = Some(CarTelemetry2::parse(&h, packet)),
            id::CAR_TELEMETRY => s.on_telemetry(&h, CarTelemetry::parse(&h, packet), &mut out),
            id::SESSION_HISTORY => s.on_history(&SessionHistory::parse(packet), &mut out),
            id::EVENT => s.on_event(Event::parse(packet), &mut out),
            _ => {}
        }
        out
    }
}

impl Decoder for F1Decoder {
    fn decode(&mut self, record: &RawRecord) -> Result<Vec<SourceEvent>, SourceError> {
        if record.kind != KIND_PACKET {
            return Err(SourceError::InvalidData(format!(
                "unknown F1 record kind {}",
                record.kind
            )));
        }
        let Some((ts, packet)) = record.payload.split_first_chunk::<8>() else {
            return Err(SourceError::InvalidData("F1 record too short".into()));
        };
        Ok(self.on_packet(i64::from_le_bytes(*ts), packet))
    }

    fn finish(&mut self) -> Vec<SourceEvent> {
        match self.s.take() {
            Some(s) if s.meta.is_some() && !s.ended => vec![SourceEvent::SessionEnd],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::SimGame;

    fn decode(d: &mut F1Decoder, packet: &[u8]) -> Vec<SourceEvent> {
        d.decode(&packet_record(1_794_000_000_000, packet)).unwrap()
    }

    fn frames(events: &[SourceEvent]) -> Vec<&Frame> {
        events
            .iter()
            .filter_map(|e| match e {
                SourceEvent::Frame(f) => Some(f),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn session_starts_once_info_and_two_frames_are_there() {
        for format in [Format::F2025, Format::F2026] {
            let mut d = F1Decoder::new();
            let mut g = SimGame::new(format);
            // telemetry before the session info: no frames
            let mut events = Vec::new();
            for p in g.frame() {
                events.extend(decode(&mut d, &p));
            }
            assert!(events.is_empty());
            events.extend(decode(&mut d, &g.session()));
            events.extend(decode(&mut d, &g.participants()));
            assert!(events.is_empty(), "the sample rate isn't known yet");
            g.time += 1.0 / 60.0;
            for p in g.frame() {
                events.extend(decode(&mut d, &p));
            }
            let SourceEvent::Session(meta) = &events[0] else {
                panic!("{events:?}")
            };
            assert_eq!(meta.session_key, "f1_25:00000000000000f1:3");
            assert_eq!(meta.track_key, "f1_25:10");
            assert_eq!(meta.track_name, "Spa");
            assert_eq!(meta.car_key, "f1_25:4/0");
            assert_eq!(meta.session_type, sy_model::SessionType::TimeTrial);
            assert_eq!(meta.sample_rate_hz, 60.0);
            assert_eq!(meta.conditions.track_temp_c, Some(32.0));
            assert_eq!(
                meta.started_at.unix_timestamp(),
                1_794_000_000 - 1,
                "receive time minus session time"
            );
            let f = frames(&events);
            assert_eq!(f.len(), 1);
            assert_eq!(f[0].values.len(), meta.channels.len());
            let value = |name: &str| {
                let i = meta.channels.iter().position(|c| c.name == name).unwrap();
                f[0].values[i]
            };
            assert!((value(names::SPEED) - 50.0).abs() < 1e-6);
            assert_eq!(value(names::THROTTLE), 1.0);
            assert_eq!(value(names::GEAR), 6.0);
            assert_eq!(
                meta.channels.iter().any(|c| c.name == "f1.overtake_active"),
                format == Format::F2026
            );
        }
    }

    #[test]
    fn starts_without_participants_after_a_while() {
        let mut d = F1Decoder::new();
        let mut g = SimGame::new(Format::F2026);
        let mut events = decode(&mut d, &g.session());
        for _ in 0..2 {
            for p in g.frame() {
                events.extend(decode(&mut d, &p));
            }
            g.time += 0.1;
        }
        assert!(events.is_empty());
        g.time += PARTICIPANTS_WAIT_S;
        for p in g.frame() {
            events.extend(decode(&mut d, &p));
        }
        let SourceEvent::Session(meta) = &events[0] else {
            panic!("{events:?}")
        };
        assert_eq!(meta.car_key, "f1_25:unknown/0");
        assert_eq!(meta.car_name, "F1");
    }
}
