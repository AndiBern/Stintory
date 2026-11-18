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
//!   drops the rewound samples and flags the lap. In time trial the game
//!   invalidates a lap with a flashback.
//! - Restart of the lap (time trial menu): the lap time goes back to 0
//!   while the session time goes on, the lap number stays and the car is
//!   put before the line. The attempt is aborted ("lap restarted"), the
//!   run-up skipped, and the lap starts again at the line.
//! - The UDP format can change within a session (game settings); the
//!   channel layout stays the one the session started with.
//! - Over the line in the pit lane outside races the game counts no lap:
//!   `LineCrossed` ends the in-lap there.
//! - Races: a lap that starts standing on the grid counts from there (some
//!   grids are after the line). A red flag cuts the lap off; nothing is
//!   recorded until the game resumes (it skips ahead to a standing restart).
//! - The session starts once the player drives (not in menus, the garage
//!   or while spectating).
//! - Before the line the game counts lap 1 with a negative lap distance.
//!   Outside races that is the out-lap from the pit exit (in time trial up
//!   to almost a lap): it becomes lap 0, flagged out-lap. In races it is
//!   the grid: lap 1's official time starts at the start, so it stays.
//! - At the end of a race the lap number doesn't go up at the flag; only
//!   the last lap time changes. That finishes the lap, and later frames
//!   (the cool-down) get the next number.
//! - Axes and signs (checked with a recording at Catalunya): world X/Z are
//!   horizontal, Y up. `pos_x` = X, `pos_y` = -Z gives an unmirrored map
//!   (steering left turns the path counter-clockwise). The game's lateral g
//!   is negative in right-hand turns (ours positive). The game's yaw is the
//!   direction of travel from +Z towards +X.
//! - Track key `f1_25:<trackId>`, car key `f1_25:<teamId>/<formula>`.

use sy_model::capture::{
    ChannelSpec, Decoder, Frame, RawRecord, SessionMeta, SourceError, SourceEvent, ValueKind,
};
use sy_model::channel::names;
use sy_model::session::Conditions;
use sy_model::{GameId, Unit};
use time::OffsetDateTime;

use crate::packets::{
    self, CarDamage, CarMotion, CarSetup, CarStatus, CarTelemetry, CarTelemetry2, Event, Format,
    Header, LapData, MotionEx, Participant, SessionHistory, id,
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
    /// Channel layout: the format the session started with. It stays when
    /// the game switches formats, so the running lap goes on.
    layout: Format,
    game_version: String,
    started_at: OffsetDateTime,
    first_time: f32,
    session: Option<packets::Session>,
    participant: Option<Participant>,
    setup: Option<CarSetup>,
    motion: Option<CarMotion>,
    lap: Option<LapData>,
    /// Session time of `lap`
    lap_time: f32,
    status: Option<CarStatus>,
    damage: Option<CarDamage>,
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
    /// On the out-lap before the line (time trial), see `on_telemetry`
    out_lap: bool,
    /// The race's last lap was finished (the game's lap number stays)
    finished_lap: Option<u8>,
    /// Reasons for invalidated laps from the game's penalties
    invalidations: Vec<InvalidationNote>,
    /// After a restart of the lap (time trial), until the car crosses the
    /// line: the run-up isn't recorded
    run_up: bool,
    /// After a red flag until the race resumes: nothing is recorded
    red_flag: bool,
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
    d: Option<&'a CarDamage>,
    session: Option<&'a packets::Session>,
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
        // world X/Z are horizontal, Y is up; X east, -Z north gives an
        // unmirrored map (checked with recordings, see the module docs)
        ch(names::POS_X, Unit::Meter, F32, |i| {
            opt(i.m, |m| f64::from(m.position[0]))
        }),
        ch(names::POS_Y, Unit::Meter, F32, |i| {
            opt(i.m, |m| -f64::from(m.position[2]))
        }),
        ch(names::POS_Z, Unit::Meter, F32, |i| {
            opt(i.m, |m| f64::from(m.position[1]))
        }),
        // the game's lateral g is negative in right-hand turns
        ch(names::G_LAT, Unit::MeterPerSecondSquared, F32, |i| {
            opt(i.m, |m| -f64::from(m.g_force[0]) * G)
        }),
        ch(names::G_LONG, Unit::MeterPerSecondSquared, F32, |i| {
            opt(i.m, |m| f64::from(m.g_force[1]) * G)
        }),
        ch(names::G_VERT, Unit::MeterPerSecondSquared, F32, |i| {
            opt(i.m, |m| f64::from(m.g_force[2]) * G)
        }),
        // the game's yaw is the heading from +Z towards +X; ours is
        // clockwise from north (-Z): π - yaw
        ch(names::YAW, Unit::Radian, F32, |i| {
            opt(i.m, |m| heading(m.yaw_pitch_roll[0]))
        }),
        ch(names::FUEL, Unit::Kilogram, F32, |i| {
            opt(i.s, |s| f64::from(s.fuel_kg))
        }),
        ch(names::BRAKE_BIAS, Unit::Ratio, F32, |i| {
            opt(i.s, |s| f64::from(s.front_brake_bias) / 100.0)
        }),
        ch("f1.drs", Unit::None, Bool, |i| f64::from(u8::from(i.t.drs))),
        // tyres (car status): the compound (actual: C1–C6, inter, wet; visual:
        // soft, medium, hard, …; codes in the UDP spec) and the set's age
        ch("f1.tyre_compound", Unit::None, F32, |i| {
            opt(i.s, |s| f64::from(s.tyre_compound))
        }),
        ch("f1.tyre_visual_compound", Unit::None, F32, |i| {
            opt(i.s, |s| f64::from(s.tyre_visual_compound))
        }),
        ch("f1.tyre_age_laps", Unit::None, F32, |i| {
            opt(i.s, |s| f64::from(s.tyre_age_laps))
        }),
        ch("f1.ers_store", Unit::Joule, F32, |i| {
            opt(i.s, |s| f64::from(s.ers_store_j))
        }),
        ch("f1.ers_deploy_mode", Unit::None, I32, |i| {
            opt(i.s, |s| f64::from(s.ers_deploy_mode))
        }),
        ch("f1.ers_power", Unit::Watt, F32, |i| {
            opt(i.s, |s| f64::from(s.mguk_power_w))
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
    // surface under each wheel (tables::surface: 0 tarmac, 1 rumble strip,
    // 4 gravel, 7 grass, …)
    v.push(ch("f1.surface_rl", Unit::None, I32, |i| {
        f64::from(i.t.surface[0])
    }));
    v.push(ch("f1.surface_rr", Unit::None, I32, |i| {
        f64::from(i.t.surface[1])
    }));
    v.push(ch("f1.surface_fl", Unit::None, I32, |i| {
        f64::from(i.t.surface[2])
    }));
    v.push(ch("f1.surface_fr", Unit::None, I32, |i| {
        f64::from(i.t.surface[3])
    }));
    // wear 0 = new … 1 = worn out (the game: percent), from the damage packet
    wheels!("f1.tyre_wear", Unit::Ratio, |i, w| opt(i.d, |d| {
        f64::from(d.tyres_wear_pct[w]) / 100.0
    }));
    // weather from the session packet (per lap in the lap table)
    v.push(ch(names::AIR_TEMP, Unit::Celsius, F32, |i| {
        opt(i.session, |s| f64::from(s.air_temp_c))
    }));
    v.push(ch(names::TRACK_TEMP, Unit::Celsius, F32, |i| {
        opt(i.session, |s| f64::from(s.track_temp_c))
    }));
    // tables::weather: 0 clear, 1 light cloud, 2 overcast, 3 light rain, …
    v.push(ch("f1.weather", Unit::None, F32, |i| {
        opt(i.session, |s| f64::from(s.weather))
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

/// Why the game invalidated a lap, from its penalty events.
#[derive(Debug, Clone)]
struct InvalidationNote {
    /// Session time of the penalty (a flashback before it takes it back)
    time: f32,
    /// The game's lap number it applies to
    lap: i32,
    reason: String,
    /// From another penalty (a warning, a stop-go, …) for track limits:
    /// used when the lap has no "lap invalidated" penalty (races send
    /// those less)
    other_penalty: bool,
}

impl SessionState {
    /// Why lap `lap` (game numbering) is invalid: the reasons of the
    /// game's penalties, or a generic text.
    fn invalid_reason(&self, lap: i32) -> String {
        let mut reasons: Vec<&str> = Vec::new();
        let notes: Vec<_> = self.invalidations.iter().filter(|n| n.lap == lap).collect();
        let direct = notes.iter().any(|n| !n.other_penalty);
        for n in notes.into_iter().filter(|n| n.other_penalty != direct) {
            if !reasons.contains(&n.reason.as_str()) {
                reasons.push(&n.reason);
            }
        }
        if reasons.is_empty() {
            "invalidated by the game".to_owned()
        } else {
            reasons.join(", ")
        }
    }

    /// Events for a finished lap: the official time (`last_ms`, sectors 1
    /// and 2 as last seen in the lap) and the game's verdict at the end of
    /// the lap.
    fn lap_completed(&self, prev: &LapData, last_ms: u32, out: &mut Vec<SourceEvent>) {
        lap_completed(
            prev,
            last_ms,
            prev.lap_invalid
                .then(|| self.invalid_reason(i32::from(prev.lap))),
            out,
        );
    }
}

fn lap_completed(
    prev: &LapData,
    last_ms: u32,
    invalid: Option<String>,
    out: &mut Vec<SourceEvent>,
) {
    let lap = i32::from(prev.lap);
    if last_ms > 0 {
        let (s1, s2) = (prev.sector1_ms, prev.sector2_ms);
        let sectors = if s1 > 0 && s2 > 0 && s1 + s2 < last_ms {
            vec![s1, s2, last_ms - s1 - s2]
        } else {
            Vec::new()
        };
        out.push(SourceEvent::LapTime {
            lap,
            lap_time_ms: last_ms,
            sector_times_ms: sectors,
        });
    }
    if let Some(reason) = invalid {
        out.push(SourceEvent::Invalidate {
            lap: Some(lap),
            reason,
        });
    }
}

/// Our heading (clockwise from `pos_y`, -π…π) from the game's yaw.
fn heading(yaw: f32) -> f64 {
    let h = std::f64::consts::PI - f64::from(yaw);
    (h + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI
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
            layout: h.format,
            game_version: format!("{}.{:02}", h.game_major, h.game_minor),
            started_at: wall - time::Duration::seconds_f64(f64::from(h.session_time).max(0.0)),
            first_time: h.session_time,
            session: None,
            participant: None,
            setup: None,
            motion: None,
            lap: None,
            lap_time: h.session_time,
            status: None,
            damage: None,
            motion_ex: None,
            telemetry2: None,
            telemetry_times: Vec::new(),
            meta: None,
            ended: false,
            last_frame_time: None,
            frame_lap: None,
            history_invalid: Vec::new(),
            out_lap: false,
            finished_lap: None,
            invalidations: Vec::new(),
            run_up: false,
            red_flag: false,
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
        // A session starts once the player drives: not in the menus before
        // a time trial, in the garage, or while spectating.
        if self.meta.is_none() && (s.spectating || self.lap.is_none_or(|l| l.driver_status == 0)) {
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
            "game_mode_name": tables::game_mode(s.game_mode),
            "rule_set": s.rule_set,
            "rule_set_name": tables::rule_set(s.rule_set),
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
            channels: channels(self.layout).into_iter().map(|c| c.spec).collect(),
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

    fn on_lap_data(&mut self, time: f32, ld: LapData, out: &mut Vec<SourceEvent>) {
        let prev = self.lap.replace(ld);
        let prev_time = std::mem::replace(&mut self.lap_time, time);
        if self.meta.is_none() || self.ended {
            return;
        }
        let Some(prev) = prev else { return };
        if self.red_flag && ld.lap != prev.lap {
            self.red_flag = false; // the race resumes (the game skips ahead)
        }
        let len = self
            .session
            .as_ref()
            .map_or(0.0, |s| f32::from(s.track_length_m));
        let wrapped = len > 0.0
            && prev.lap_distance_m > len - 200.0
            && (0.0..200.0).contains(&ld.lap_distance_m);
        let on_track = ld.pit_status == 0 && prev.pit_status == 0;
        if ld.lap == prev.lap
            && ld.last_lap_ms == prev.last_lap_ms
            && wrapped
            && !on_track
            && !self.run_up
            && self.finished_lap.is_none()
        {
            // Over the line in the pit lane, and the game didn't count a
            // lap (practice, qualifying): an in-lap without a time.
            if prev.lap_invalid {
                out.push(SourceEvent::Invalidate {
                    lap: Some(i32::from(prev.lap)),
                    reason: self.invalid_reason(i32::from(prev.lap)),
                });
            }
            self.invalidations.retain(|n| n.lap != i32::from(prev.lap));
            if self.frame_lap.is_some() {
                out.push(SourceEvent::LineCrossed);
            }
        } else if ld.lap == prev.lap
            && ld.last_lap_ms == prev.last_lap_ms
            && on_track
            && time > prev_time
            && ld.current_lap_ms + 1000 < prev.current_lap_ms
        {
            // The lap time went back to 0 while session time went on: the
            // player restarted the lap (time trial). The game puts the car
            // before the line, keeps the lap number and starts the lap
            // again at the line. (A flashback also sets the lap time back,
            // but with the session time.)
            self.restart_lap(ld.lap, out);
        }
        if ld.lap == prev.lap + 1 {
            self.lap_completed(&prev, ld.last_lap_ms, out);
        } else if ld.lap > prev.lap && self.frame_lap.is_some() {
            // laps skipped (after a red flag): the time isn't this lap's
            out.push(SourceEvent::LapAborted {
                reason: "laps skipped by the game".into(),
            });
            self.frame_lap = None;
        } else if ld.lap == prev.lap
            && ld.last_lap_ms != prev.last_lap_ms
            && ld.last_lap_ms > 0
            && self.finished_lap.is_none()
        {
            // The race's last lap: the game doesn't count up at the flag,
            // only the last lap time changes. Later frames go to the next
            // lap number, so the recorder closes this lap.
            self.lap_completed(&prev, ld.last_lap_ms, out);
            self.finished_lap = Some(ld.lap);
        }
        if ld.driver_status == 0 {
            self.run_up = false; // back in the garage, the next lap is an out-lap
        }
        if ld.driver_status == 0 && prev.driver_status != 0 && self.frame_lap.is_some() {
            out.push(SourceEvent::LapAborted {
                reason: "returned to the garage".into(),
            });
            self.frame_lap = None;
        }
    }

    /// The lap `lap` (game numbering) starts again: the attempt so far is
    /// incomplete and its penalties don't count.
    fn restart_lap(&mut self, lap: u8, out: &mut Vec<SourceEvent>) {
        self.invalidations.retain(|n| n.lap < i32::from(lap));
        self.run_up = true;
        if self.frame_lap.is_some() {
            out.push(SourceEvent::LapAborted {
                reason: "lap restarted".into(),
            });
            self.frame_lap = None;
        }
    }

    /// The game switched between UDP formats 2025 and 2026 (possible in its
    /// settings while driving). The session and its channel layout go on
    /// (channels only in 2026 stay empty in 2025).
    fn switch_format(&mut self, format: Format) {
        tracing::info!(from = ?self.format, to = ?format, "UDP format changed within the session");
        self.format = format;
        self.motion = None;
        self.status = None;
        self.motion_ex = None;
        self.telemetry2 = None;
    }

    fn is_race(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| tables::session_type(s.session_type) == sy_model::SessionType::Race)
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
                    reason: self.invalid_reason(i32::from(number)),
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
        if self.session.as_ref().is_some_and(|s| s.spectating)
            || ld.driver_status == 0
            || self.red_flag
        {
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
            d: self.damage.as_ref(),
            session: self.session.as_ref(),
            x: self.motion_ex.as_ref(),
            t2: self.telemetry2.as_ref(),
        };
        let values = channels(self.layout)
            .iter()
            .map(|c| (c.get)(&inputs))
            .collect();
        let mut lap = i32::from(ld.lap);
        let mut lap_dist = ld.lap_distance_m;
        let mut in_pits = ld.pit_status != 0;
        // standing on the grid (start, restart after a red flag)
        let standing_start = self.is_race() && !in_pits && t.speed_kmh < 2;
        if self.finished_lap == Some(ld.lap) {
            lap += 1; // after the flag
        }
        // Before the line the game counts lap 1 with a negative distance.
        // Outside races that's the out-lap from the pit exit (it can be
        // almost a lap): we make it lap 0, an out-lap, with the distance
        // from the line. In races it is the grid, and lap 1's time starts
        // there.
        let len = self
            .session
            .as_ref()
            .map_or(0.0, |s| f32::from(s.track_length_m));
        if self.run_up {
            if ld.current_lap_ms > 0 && lap_dist < len / 2.0 {
                self.run_up = false; // crossed the line
            } else {
                return;
            }
        }
        if !self.out_lap && len > 0.0 && lap_dist < 0.0 && !self.is_race() {
            self.out_lap = true;
            in_pits = true; // the first sample: flags the out-lap
        }
        if self.out_lap {
            if lap_dist >= 0.0 {
                self.out_lap = false;
            } else {
                lap -= 1;
                lap_dist += len;
            }
        }
        self.frame_lap = Some(lap);
        out.push(SourceEvent::Frame(Frame {
            session_time: time,
            lap,
            lap_dist,
            in_pits,
            standing_start,
            values,
        }));
    }

    fn on_event(&mut self, h: &Header, event: Event, out: &mut Vec<SourceEvent>) {
        match event {
            Event::Penalty {
                penalty_type,
                infringement,
                car,
                lap,
            } if car == self.player => {
                if let Some(reason) = tables::invalidation_reason(penalty_type, infringement) {
                    for offset in tables::invalidated_laps(penalty_type) {
                        self.invalidations.push(InvalidationNote {
                            time: h.session_time,
                            lap: i32::from(lap) + offset,
                            reason: reason.clone(),
                            other_penalty: false,
                        });
                    }
                } else if let Some(reason) = tables::track_limits(infringement) {
                    self.invalidations.push(InvalidationNote {
                        time: h.session_time,
                        lap: i32::from(lap),
                        reason: reason.to_owned(),
                        other_penalty: true,
                    });
                }
            }
            Event::Flashback { session_time, .. } if self.meta.is_some() => {
                // what happened after the flashback point didn't happen
                self.invalidations.retain(|n| n.time < session_time);
                let to = f64::from(session_time);
                out.push(SourceEvent::Rewind {
                    to_session_time: to,
                });
                self.last_frame_time = Some(to - 1e-6);
            }
            Event::RedFlag if self.meta.is_some() => {
                // the game stops the race and later skips ahead to the
                // restart from the grid
                if self.frame_lap.is_some() {
                    out.push(SourceEvent::LapAborted {
                        reason: "red flag".into(),
                    });
                    self.frame_lap = None;
                }
                self.red_flag = true;
            }
            Event::LightsOut => self.red_flag = false,
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
            s.switch_format(h.format);
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
            id::LAP_DATA => s.on_lap_data(h.session_time, LapData::parse(&h, packet), &mut out),
            id::MOTION => s.motion = Some(CarMotion::parse(&h, packet)),
            id::CAR_STATUS => s.status = Some(CarStatus::parse(&h, packet)),
            id::CAR_DAMAGE => s.damage = Some(CarDamage::parse(&h, packet)),
            id::MOTION_EX => s.motion_ex = Some(MotionEx::parse(packet)),
            id::CAR_TELEMETRY_2 => s.telemetry2 = Some(CarTelemetry2::parse(&h, packet)),
            id::CAR_TELEMETRY => s.on_telemetry(&h, CarTelemetry::parse(&h, packet), &mut out),
            id::SESSION_HISTORY => s.on_history(&SessionHistory::parse(packet), &mut out),
            id::EVENT => s.on_event(&h, Event::parse(packet), &mut out),
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
