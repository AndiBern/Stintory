// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Synthetic LMU data, for tests.
//!
//! [`SimLmu`] drives a car round a circular track with a speed that varies
//! along the lap, and produces the records the live source would spool:
//! telemetry at 50 Hz and scoring five times a second, laid out as in
//! [`crate::layout`]. It is not a recording of the real game.

use sy_model::capture::RawRecord;

use crate::layout::{BytesMut, SCORING_INFO_SIZE, TELEM_SIZE, VEH_SCORING_SIZE, WHEEL_SIZE, off};
use crate::records::{scoring_record, setup_record, telemetry_record};

/// Telemetry interval, s.
pub const TELEMETRY_DT: f64 = 0.02;
/// Scoring interval, s.
pub const SCORING_DT: f64 = 0.2;
/// Integration step, s.
const SUBSTEP: f64 = 0.001;

/// A simulated car and session.
#[derive(Debug, Clone)]
pub struct SimLmu {
    /// Track length, m
    pub track_length: f64,
    /// Average speed, m/s
    pub speed: f64,
    /// Elapsed session time, s
    pub et: f64,
    /// Distance from the line, m
    pub dist: f64,
    /// Telemetry lap number
    pub lap: i32,
    /// Completed laps (scoring)
    pub total_laps: i16,
    /// Start of the current lap, s
    pub lap_start_et: f64,
    /// Time of the last completed lap (−1: none)
    pub last_lap_time: f64,
    /// Sector 1 and sectors 1 + 2 of the last lap
    pub last_sectors: (f64, f64),
    /// In the pit lane
    pub in_pits: bool,
    /// Distance of the pit exit (the car leaves the pit lane there)
    pub pit_exit: f64,
    /// The game invalidated the current lap
    pub lap_invalidated: bool,
    /// Track-limits steps
    pub track_limits_steps: u8,
    /// Driving (not in the garage)
    pub in_realtime: bool,
    /// Session number (10 = race 1)
    pub session: i32,
    /// Wall clock at `et` = 0, Unix ms
    pub epoch_ms: i64,
    /// Livery
    pub vehicle_name: String,
    /// Model
    pub vehicle_model: String,
    /// True distance at every telemetry sample: (elapsed time, distance)
    pub truth: Vec<(f64, f64)>,
    /// A lap's time is not counted by the game (`mLastLapTime` = −1)
    pub untimed_next_lap: bool,
    cur_sectors: (f64, f64),
    next_scoring: f64,
    next_telemetry: f64,
}

impl Default for SimLmu {
    fn default() -> Self {
        Self::new()
    }
}

impl SimLmu {
    /// A practice session at a 2 km track, the car in the pit lane 300 m
    /// before the line; the pit exit is 150 m before the line.
    pub fn new() -> Self {
        Self {
            track_length: 2000.0,
            speed: 50.0,
            et: 30.0,
            dist: 1700.0,
            lap: 0,
            total_laps: 0,
            lap_start_et: 30.0,
            last_lap_time: -1.0,
            last_sectors: (-1.0, -1.0),
            in_pits: true,
            pit_exit: 1850.0,
            lap_invalidated: false,
            track_limits_steps: 0,
            in_realtime: true,
            session: 1,
            epoch_ms: 1_794_000_000_000,
            vehicle_name: "Proton Competition 2025 #99".into(),
            vehicle_model: "Porsche 963".into(),
            truth: Vec::new(),
            untimed_next_lap: false,
            cur_sectors: (-1.0, -1.0),
            next_scoring: 30.0,
            next_telemetry: 30.0,
        }
    }

    /// Speed at a distance: ±40 % around the average, three times a lap.
    pub fn speed_at(&self, dist: f64) -> f64 {
        let phase = dist / self.track_length * std::f64::consts::TAU * 3.0;
        self.speed * (1.0 + 0.4 * phase.sin())
    }

    /// Drives for `seconds`, returning the records the source would spool.
    pub fn drive(&mut self, seconds: f64) -> Vec<RawRecord> {
        let mut out = Vec::new();
        let end = self.et + seconds;
        while self.et < end - 1e-9 {
            if self.et + 1e-9 >= self.next_scoring {
                out.push(self.scoring());
                self.next_scoring += SCORING_DT;
            }
            if self.et + 1e-9 >= self.next_telemetry {
                if self.in_realtime {
                    self.truth.push((self.et, self.dist));
                    out.push(telemetry_record(&self.telemetry()));
                }
                self.next_telemetry += TELEMETRY_DT;
            }
            self.substep();
        }
        out
    }

    /// Drives until the line was crossed, then `extra` seconds.
    pub fn drive_lap(&mut self, extra: f64) -> Vec<RawRecord> {
        let lap = self.lap;
        let mut out = Vec::new();
        while self.lap == lap {
            out.extend(self.drive(0.1));
        }
        out.extend(self.drive(extra));
        out
    }

    /// A setup record.
    pub fn setup(json: &str) -> RawRecord {
        setup_record(json.as_bytes())
    }

    fn substep(&mut self) {
        if !self.in_realtime {
            self.et += SUBSTEP;
            return;
        }
        let v = self.speed_at(self.dist);
        let before = self.dist;
        self.dist += v * SUBSTEP;
        let t0 = self.et;
        self.et += SUBSTEP;
        if self.in_pits && before < self.pit_exit && self.dist >= self.pit_exit {
            self.in_pits = false;
        }
        let third = self.track_length / 3.0;
        let cross = |mark: f64| t0 + (mark - before) / v;
        if before < third && self.dist >= third {
            self.cur_sectors.0 = cross(third) - self.lap_start_et;
        }
        if before < 2.0 * third && self.dist >= 2.0 * third {
            self.cur_sectors.1 = cross(2.0 * third) - self.lap_start_et;
        }
        if self.dist >= self.track_length {
            let at = cross(self.track_length);
            let timed = self.lap > 0 && !std::mem::take(&mut self.untimed_next_lap);
            self.last_lap_time = if timed { at - self.lap_start_et } else { -1.0 };
            self.last_sectors = if timed {
                self.cur_sectors
            } else {
                (-1.0, -1.0)
            };
            self.cur_sectors = (-1.0, -1.0);
            self.dist -= self.track_length;
            self.lap += 1;
            self.total_laps += 1;
            self.lap_start_et = at;
            self.lap_invalidated = false;
            self.in_pits = false;
        }
    }

    /// Position on the circle (clockwise seen from above).
    fn position(&self) -> ([f64; 3], [[f64; 3]; 3]) {
        let r = self.track_length / std::f64::consts::TAU;
        let th = self.dist / r;
        let pos = [r * th.sin(), 0.0, r * th.cos()];
        // forward (local -z) = (cos θ, 0, -sin θ) in world x/z
        let (s, c) = (-th.cos(), th.sin());
        let ori = [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]];
        (pos, ori)
    }

    /// The player's `TelemInfoV01`.
    pub fn telemetry(&self) -> Vec<u8> {
        use off::telem as o;
        let mut buf = vec![0u8; TELEM_SIZE];
        let mut b = BytesMut(&mut buf);
        let v = self.speed_at(self.dist);
        let (pos, ori) = self.position();
        b.i32(o::ID, 7);
        b.f64(o::DELTA_TIME, TELEMETRY_DT);
        b.f64(o::ELAPSED_TIME, self.et);
        b.i32(o::LAP_NUMBER, self.lap);
        b.f64(o::LAP_START_ET, self.lap_start_et);
        b.str(o::VEHICLE_NAME, 64, &self.vehicle_name);
        b.str(o::TRACK_NAME, 64, "Sim Oval");
        b.vect3(o::POS, pos);
        b.vect3(o::LOCAL_VEL, [0.0, 0.0, -v]);
        b.vect3(o::LOCAL_ACCEL, [-v * v / 318.0, 0.0, 0.0]);
        for (i, row) in ori.iter().enumerate() {
            b.vect3(o::ORI + 24 * i, *row);
        }
        b.i32(o::GEAR, 4);
        b.f64(o::ENGINE_RPM, 7000.0);
        b.f64(o::THROTTLE, 0.8);
        b.f64(o::BRAKE, 0.0);
        b.f64(o::STEERING, 0.1);
        b.f64(o::FUEL, 60.0 - self.et * 0.01);
        b.f64(o::REAR_BRAKE_BIAS, 0.45);
        b.f32(o::STEERING_RANGE, 400.0);
        b.i32(o::CURRENT_SECTOR, if self.in_pits { i32::MIN } else { 0 });
        b.u8(o::LAP_INVALIDATED, u8::from(self.lap_invalidated));
        b.u8(o::ABS_ACTIVE, u8::from(self.dist % 500.0 < 20.0));
        b.u8(o::TC_ACTIVE, u8::from(self.dist % 500.0 > 300.0));
        b.u8(o::TC, 5);
        b.u8(o::ABS, 3);
        b.u8(o::TRACK_LIMITS_STEPS, self.track_limits_steps);
        b.f32(o::VIRTUAL_ENERGY, (0.9 - self.et as f32 * 0.0005).max(0.0));
        b.str(o::VEHICLE_MODEL, 30, &self.vehicle_model);
        b.u8(o::VEHICLE_CLASS, 0);
        b.u8(o::VEHICLE_CHAMPIONSHIP, 2);
        for w in 0..4 {
            let at = o::WHEEL + w * WHEEL_SIZE;
            b.f64(at + off::wheel::PRESSURE, 170.0);
            b.vect3(at + off::wheel::TEMPERATURE, [353.15, 358.15, 363.15]);
            b.f64(at + off::wheel::BRAKE_TEMP, 400.0);
        }
        buf
    }

    /// The scoring record (`ScoringInfoV01` + the player's scoring).
    pub fn scoring(&self) -> RawRecord {
        let mut info = vec![0u8; SCORING_INFO_SIZE];
        {
            use off::scoring as o;
            let mut b = BytesMut(&mut info);
            b.str(o::TRACK_NAME, 64, "Sim Oval");
            b.i32(o::SESSION, self.session);
            b.f64(o::CURRENT_ET, self.et);
            b.f64(o::END_ET, 3600.0);
            b.f64(o::LAP_DIST, self.track_length);
            b.i32(o::NUM_VEHICLES, 20);
            b.u8(o::GAME_PHASE, 5);
            b.u8(o::IN_REALTIME, u8::from(self.in_realtime));
            b.f64(o::AMBIENT_TEMP, 21.0);
            b.f64(o::TRACK_TEMP, 30.0);
            b.str(o::PLAYER_NAME, 32, "Sim Driver");
            b.str(o::SERVER_NAME, 32, "Some Server");
        }
        let mut veh = vec![0u8; VEH_SCORING_SIZE];
        {
            use off::veh as o;
            let mut b = BytesMut(&mut veh);
            b.i32(o::ID, 7);
            b.str(o::DRIVER_NAME, 32, "Sim Driver");
            b.str(o::VEHICLE_NAME, 64, &self.vehicle_name);
            b.i16(o::TOTAL_LAPS, self.total_laps);
            b.f64(o::LAP_DIST, self.dist);
            b.f64(o::LAST_SECTOR1, self.last_sectors.0);
            b.f64(o::LAST_SECTOR2, self.last_sectors.1);
            b.f64(o::LAST_LAP_TIME, self.last_lap_time);
            b.u8(o::IS_PLAYER, 1);
            b.u8(o::IN_PITS, u8::from(self.in_pits));
            b.f64(o::LAP_START_ET, self.lap_start_et);
            b.str(o::VEHICLE_CLASS, 32, "Hypercar");
            b.put(o::STEAM_ID, &76_561_198_000_000_000u64.to_le_bytes());
        }
        let received = self.epoch_ms + (self.et * 1000.0) as i64;
        scoring_record(received, 1_200, &info, &veh)
    }
}
