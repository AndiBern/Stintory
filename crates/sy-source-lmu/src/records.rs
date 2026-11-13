// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The raw records the LMU source spools, and their parsed form.
//!
//! Only the player's car is spooled (docs/PLAN.md §5.2), never the whole
//! `LMU_Data` mapping:
//!
//! | kind | payload |
//! |---|---|
//! | [`KIND_TELEMETRY`] | the player's `TelemInfoV01` (1888 bytes) |
//! | [`KIND_SCORING`] | receive time (Unix ms, `i64` LE), game version (`i32` LE), `ScoringInfoV01` (548 bytes), the player's `VehicleScoringInfoV01` (584 bytes) |
//! | [`KIND_SETUP`] | the player's car setup as JSON, from the game's REST API |
//!
//! Before a scoring record is built, the pointers, the server's address and
//! name, the player name and the Steam id are zeroed: none of it is needed.
//! The receive time makes the session's wall-clock start reproducible when
//! a spool file is replayed.

use sy_model::capture::{RawRecord, SourceError};

use crate::layout::{
    Bytes, BytesMut, SCORING_INFO_SIZE, TELEM_SIZE, VEH_SCORING_SIZE, WHEEL_SIZE, off,
};

/// Raw record: the player's telemetry.
pub const KIND_TELEMETRY: u8 = 1;
/// Raw record: session scoring and the player's scoring.
pub const KIND_SCORING: u8 = 2;
/// Raw record: car setup (JSON).
pub const KIND_SETUP: u8 = 3;

const SCORING_HEADER: usize = 8 + 4;
const SCORING_RECORD_LEN: usize = SCORING_HEADER + SCORING_INFO_SIZE + VEH_SCORING_SIZE;

/// Builds a [`KIND_TELEMETRY`] record from the player's `TelemInfoV01`.
///
/// # Panics
///
/// If `telem` is not [`TELEM_SIZE`] bytes long.
pub fn telemetry_record(telem: &[u8]) -> RawRecord {
    assert_eq!(telem.len(), TELEM_SIZE, "TelemInfoV01");
    RawRecord {
        kind: KIND_TELEMETRY,
        payload: telem.to_vec(),
    }
}

/// Builds a [`KIND_SCORING`] record from `ScoringInfoV01` and the player's
/// `VehicleScoringInfoV01`, removing what isn't needed.
///
/// # Panics
///
/// If the parts don't have their struct sizes.
pub fn scoring_record(
    received_unix_ms: i64,
    game_version: i32,
    info: &[u8],
    player: &[u8],
) -> RawRecord {
    assert_eq!(info.len(), SCORING_INFO_SIZE, "ScoringInfoV01");
    assert_eq!(player.len(), VEH_SCORING_SIZE, "VehicleScoringInfoV01");
    let mut payload = Vec::with_capacity(SCORING_RECORD_LEN);
    payload.extend_from_slice(&received_unix_ms.to_le_bytes());
    payload.extend_from_slice(&game_version.to_le_bytes());
    payload.extend_from_slice(info);
    payload.extend_from_slice(player);
    {
        let mut i = BytesMut(&mut payload[SCORING_HEADER..SCORING_HEADER + SCORING_INFO_SIZE]);
        i.zero(off::scoring::RESULTS_STREAM, 8);
        i.zero(off::scoring::VEHICLE, 8);
        i.zero(off::scoring::SERVER_PUBLIC_IP, 4);
        i.zero(off::scoring::SERVER_PORT, 2);
        i.zero(off::scoring::SERVER_NAME, 32);
        i.zero(off::scoring::PLAYER_NAME, 32);
        i.zero(off::scoring::PLR_FILE_NAME, 64);
    }
    {
        let mut p = BytesMut(&mut payload[SCORING_HEADER + SCORING_INFO_SIZE..]);
        p.zero(off::veh::STEAM_ID, 8);
    }
    RawRecord {
        kind: KIND_SCORING,
        payload,
    }
}

/// Builds a [`KIND_SETUP`] record.
pub fn setup_record(json: &[u8]) -> RawRecord {
    RawRecord {
        kind: KIND_SETUP,
        payload: json.to_vec(),
    }
}

/// One wheel of [`Telemetry`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Wheel {
    /// Suspension deflection, m
    pub suspension_deflection: f64,
    /// Ride height, m
    pub ride_height: f64,
    /// Brake temperature, °C
    pub brake_temp_c: f64,
    /// Tyre pressure, kPa
    pub pressure_kpa: f64,
    /// Tread temperatures left/centre/right, K
    pub temperature_k: [f64; 3],
    /// Carcass temperature, K
    pub carcass_temp_k: f64,
    /// Tread left, 1 new (the header: "wear (0.0-1.0, fraction of
    /// maximum)"; recordings count down from 1)
    pub wear: f64,
    /// 0 dry, 1 wet, 2 grass, 3 dirt, 4 gravel, 5 rumble strip, 6 special
    pub surface_type: u8,
    /// The tyre is flat (puncture)
    pub flat: bool,
    /// The wheel came off
    pub detached: bool,
    /// The tyre's compound (`mCompoundIndex`, per wheel)
    pub compound_index: u8,
    /// `mCompoundType`
    pub compound_type: u8,
}

/// The player's telemetry (`TelemInfoV01`), the fields Stintory uses.
#[derive(Debug, Clone, Default, PartialEq)]
#[allow(missing_docs)] // field names and units follow InternalsPlugin.hpp
pub struct Telemetry {
    pub id: i32,
    pub delta_time: f64,
    pub elapsed_time: f64,
    pub lap_number: i32,
    pub lap_start_et: f64,
    pub vehicle_name: String,
    pub track_name: String,
    /// World position, m (left-handed, +y up)
    pub pos: [f64; 3],
    /// Velocity in car coordinates (+x left, +y up, +z back), m/s
    pub local_vel: [f64; 3],
    /// Acceleration in car coordinates, m/s²
    pub local_accel: [f64; 3],
    /// Rows of the orientation matrix (car → world)
    pub ori: [[f64; 3]; 3],
    /// Rotation in car coordinates, rad/s (+y yaws right)
    pub local_rot: [f64; 3],
    pub gear: i32,
    pub engine_rpm: f64,
    pub water_temp_c: f64,
    pub oil_temp_c: f64,
    /// Driver inputs before aids: throttle, brake, clutch 0–1, steering −1…1
    pub throttle: f64,
    pub brake: f64,
    pub steering: f64,
    pub clutch: f64,
    pub front_ride_height: f64,
    pub rear_ride_height: f64,
    pub drag: f64,
    pub front_downforce: f64,
    pub rear_downforce: f64,
    /// l
    pub fuel: f64,
    pub engine_max_rpm: f64,
    pub engine_torque: f64,
    /// Zero-based sector; the sign bit is set in the pit lane
    pub current_sector: i32,
    pub speed_limiter: bool,
    pub max_gears: u8,
    pub fuel_capacity: f64,
    pub front_compound: String,
    pub rear_compound: String,
    /// Compound index (within the tyre brand) of `front_compound`
    pub front_compound_index: u8,
    pub rear_compound_index: u8,
    /// Fraction of braking on the rear axle
    pub rear_brake_bias: f64,
    pub turbo_boost: f64,
    /// Physical steering wheel range, degrees
    pub steering_range_deg: f32,
    pub battery: f64,
    pub motor_torque: f64,
    pub motor_rpm: f64,
    pub motor_state: u8,
    pub lap_invalidated: bool,
    pub abs_active: bool,
    pub tc_active: bool,
    pub tc: u8,
    pub tc_slip: u8,
    pub tc_cut: u8,
    pub abs: u8,
    pub motor_map: u8,
    pub migration: u8,
    pub front_arb: u8,
    pub rear_arb: u8,
    pub lift_and_coast: u8,
    pub track_limits_steps: u8,
    /// kW
    pub regen_kw: f32,
    pub virtual_energy: f32,
    pub vehicle_model: String,
    pub vehicle_class: u8,
    pub vehicle_championship: u8,
    /// front left, front right, rear left, rear right
    pub wheels: [Wheel; 4],
}

fn check_len(payload: &[u8], len: usize, what: &str) -> Result<(), SourceError> {
    if payload.len() == len {
        Ok(())
    } else {
        Err(SourceError::InvalidData(format!(
            "LMU {what}: {} bytes, expected {len}",
            payload.len()
        )))
    }
}

impl Telemetry {
    /// Parses the payload of a [`KIND_TELEMETRY`] record.
    pub fn parse(payload: &[u8]) -> Result<Self, SourceError> {
        use off::telem as o;
        check_len(payload, TELEM_SIZE, "telemetry")?;
        let b = Bytes(payload);
        let v3 = |at| b.vect3(at);
        let wheel = |i: usize| {
            use off::wheel as w;
            let at = o::WHEEL + i * WHEEL_SIZE;
            Wheel {
                suspension_deflection: b.f64(at + w::SUSPENSION_DEFLECTION),
                ride_height: b.f64(at + w::RIDE_HEIGHT),
                brake_temp_c: b.f64(at + w::BRAKE_TEMP),
                pressure_kpa: b.f64(at + w::PRESSURE),
                temperature_k: v3(at + w::TEMPERATURE),
                carcass_temp_k: b.f64(at + w::CARCASS_TEMP),
                wear: b.f64(at + w::WEAR),
                surface_type: b.u8(at + w::SURFACE_TYPE),
                flat: b.u8(at + w::FLAT) != 0,
                detached: b.u8(at + w::DETACHED) != 0,
                compound_index: b.u8(at + w::COMPOUND_INDEX),
                compound_type: b.u8(at + w::COMPOUND_TYPE),
            }
        };
        Ok(Self {
            id: b.i32(o::ID),
            delta_time: b.f64(o::DELTA_TIME),
            elapsed_time: b.f64(o::ELAPSED_TIME),
            lap_number: b.i32(o::LAP_NUMBER),
            lap_start_et: b.f64(o::LAP_START_ET),
            vehicle_name: b.str(o::VEHICLE_NAME, 64),
            track_name: b.str(o::TRACK_NAME, 64),
            pos: v3(o::POS),
            local_vel: v3(o::LOCAL_VEL),
            local_accel: v3(o::LOCAL_ACCEL),
            ori: [v3(o::ORI), v3(o::ORI + 24), v3(o::ORI + 48)],
            local_rot: v3(o::LOCAL_ROT),
            gear: b.i32(o::GEAR),
            engine_rpm: b.f64(o::ENGINE_RPM),
            water_temp_c: b.f64(o::WATER_TEMP),
            oil_temp_c: b.f64(o::OIL_TEMP),
            throttle: b.f64(o::THROTTLE),
            brake: b.f64(o::BRAKE),
            steering: b.f64(o::STEERING),
            clutch: b.f64(o::CLUTCH),
            front_ride_height: b.f64(o::FRONT_RIDE_HEIGHT),
            rear_ride_height: b.f64(o::REAR_RIDE_HEIGHT),
            drag: b.f64(o::DRAG),
            front_downforce: b.f64(o::FRONT_DOWNFORCE),
            rear_downforce: b.f64(o::REAR_DOWNFORCE),
            fuel: b.f64(o::FUEL),
            engine_max_rpm: b.f64(o::ENGINE_MAX_RPM),
            engine_torque: b.f64(o::ENGINE_TORQUE),
            current_sector: b.i32(o::CURRENT_SECTOR),
            speed_limiter: b.bool(o::SPEED_LIMITER),
            max_gears: b.u8(o::MAX_GEARS),
            fuel_capacity: b.f64(o::FUEL_CAPACITY),
            front_compound: b.str(o::FRONT_COMPOUND, 18),
            rear_compound: b.str(o::REAR_COMPOUND, 18),
            front_compound_index: b.u8(o::FRONT_COMPOUND_INDEX),
            rear_compound_index: b.u8(o::REAR_COMPOUND_INDEX),
            rear_brake_bias: b.f64(o::REAR_BRAKE_BIAS),
            turbo_boost: b.f64(o::TURBO_BOOST),
            steering_range_deg: b.f32(o::STEERING_RANGE),
            battery: b.f64(o::BATTERY),
            motor_torque: b.f64(o::MOTOR_TORQUE),
            motor_rpm: b.f64(o::MOTOR_RPM),
            motor_state: b.u8(o::MOTOR_STATE),
            lap_invalidated: b.bool(o::LAP_INVALIDATED),
            abs_active: b.bool(o::ABS_ACTIVE),
            tc_active: b.bool(o::TC_ACTIVE),
            tc: b.u8(o::TC),
            tc_slip: b.u8(o::TC_SLIP),
            tc_cut: b.u8(o::TC_CUT),
            abs: b.u8(o::ABS),
            motor_map: b.u8(o::MOTOR_MAP),
            migration: b.u8(o::MIGRATION),
            front_arb: b.u8(o::FRONT_ARB),
            rear_arb: b.u8(o::REAR_ARB),
            lift_and_coast: b.u8(o::LIFT_AND_COAST),
            track_limits_steps: b.u8(o::TRACK_LIMITS_STEPS),
            regen_kw: b.f32(o::REGEN),
            virtual_energy: b.f32(o::VIRTUAL_ENERGY),
            vehicle_model: b.str(o::VEHICLE_MODEL, 30),
            vehicle_class: b.u8(o::VEHICLE_CLASS),
            vehicle_championship: b.u8(o::VEHICLE_CHAMPIONSHIP),
            wheels: [wheel(0), wheel(1), wheel(2), wheel(3)],
        })
    }

    /// Speed, m/s.
    pub fn speed(&self) -> f64 {
        let [x, y, z] = self.local_vel;
        (x * x + y * y + z * z).sqrt()
    }

    /// In the pit lane (sign bit of the sector).
    pub fn in_pit_lane(&self) -> bool {
        self.current_sector < 0
    }
}

/// The player's scoring (`VehicleScoringInfoV01`), the fields Stintory uses.
#[derive(Debug, Clone, Default, PartialEq)]
#[allow(missing_docs)] // field names and units follow InternalsPlugin.hpp
pub struct VehicleScoring {
    pub id: i32,
    pub driver_name: String,
    pub vehicle_name: String,
    /// Laps completed
    pub total_laps: i16,
    /// Distance from the line, m (updated 5 times a second)
    pub lap_dist: f64,
    /// Sector 1 of the last lap, s
    pub last_sector1: f64,
    /// Sectors 1 + 2 of the last lap, s
    pub last_sector2: f64,
    /// Last lap time, s (≤ 0: none)
    pub last_lap_time: f64,
    pub is_player: bool,
    /// −1 nobody, 0 local player, 1 local AI, 2 remote, 3 replay
    pub control: i8,
    pub in_pits: bool,
    pub place: u8,
    pub vehicle_class: String,
    pub lap_start_et: f64,
    /// 0 none, 1 request, 2 entering, 3 stopped, 4 exiting
    pub pit_state: u8,
    /// 0 lap and time not counted, 1 lap counted, 2 lap and time counted
    pub count_lap_flag: u8,
    pub veh_filename: String,
}

impl VehicleScoring {
    fn parse(b: &Bytes<'_>) -> Self {
        use off::veh as o;
        Self {
            id: b.i32(o::ID),
            driver_name: b.str(o::DRIVER_NAME, 32),
            vehicle_name: b.str(o::VEHICLE_NAME, 64),
            total_laps: b.i16(o::TOTAL_LAPS),
            lap_dist: b.f64(o::LAP_DIST),
            last_sector1: b.f64(o::LAST_SECTOR1),
            last_sector2: b.f64(o::LAST_SECTOR2),
            last_lap_time: b.f64(o::LAST_LAP_TIME),
            is_player: b.bool(o::IS_PLAYER),
            control: b.i8(o::CONTROL),
            in_pits: b.bool(o::IN_PITS),
            place: b.u8(o::PLACE),
            vehicle_class: b.str(o::VEHICLE_CLASS, 32),
            lap_start_et: b.f64(o::LAP_START_ET),
            pit_state: b.u8(o::PIT_STATE),
            count_lap_flag: b.u8(o::COUNT_LAP_FLAG),
            veh_filename: b.str(o::VEH_FILENAME, 32),
        }
    }
}

/// A [`KIND_SCORING`] record: the session and the player's scoring.
#[derive(Debug, Clone, Default, PartialEq)]
#[allow(missing_docs)] // field names and units follow InternalsPlugin.hpp
pub struct Scoring {
    /// When the record was read, Unix ms
    pub received_unix_ms: i64,
    pub game_version: i32,
    pub track_name: String,
    /// 0 test day, 1–4 practice, 5–8 qualifying, 9 warm-up, 10–13 race
    pub session: i32,
    pub current_et: f64,
    pub end_et: f64,
    pub max_laps: i32,
    /// Track length, m
    pub track_length: f64,
    pub num_vehicles: i32,
    /// 0 before the session … 5 green, 6 full course yellow, 7 stopped,
    /// 8 over, 9 paused
    pub game_phase: u8,
    /// Driving (as opposed to in the garage / at the monitor)
    pub in_realtime: bool,
    pub dark_cloud: f64,
    pub raining: f64,
    /// Wetness of the racing line, 0–1
    pub min_path_wetness: f64,
    pub max_path_wetness: f64,
    pub avg_path_wetness: f64,
    pub ambient_temp_c: f64,
    pub track_temp_c: f64,
    /// 0 offline, 1 server, 2 client, 3 server and client
    pub game_mode: u8,
    pub max_players: i32,
    pub is_fixed_setup: bool,
    pub track_grip_level: u8,
    pub track_limits_steps_per_penalty: u8,
    pub track_limits_steps_per_point: u8,
    pub player: VehicleScoring,
}

impl Scoring {
    /// Parses the payload of a [`KIND_SCORING`] record.
    pub fn parse(payload: &[u8]) -> Result<Self, SourceError> {
        use off::scoring as o;
        check_len(payload, SCORING_RECORD_LEN, "scoring")?;
        let head = Bytes(&payload[..SCORING_HEADER]);
        let b = Bytes(&payload[SCORING_HEADER..SCORING_HEADER + SCORING_INFO_SIZE]);
        let p = Bytes(&payload[SCORING_HEADER + SCORING_INFO_SIZE..]);
        Ok(Self {
            received_unix_ms: i64::from_le_bytes(payload[..8].try_into().expect("8 bytes")),
            game_version: head.i32(8),
            track_name: b.str(o::TRACK_NAME, 64),
            session: b.i32(o::SESSION),
            current_et: b.f64(o::CURRENT_ET),
            end_et: b.f64(o::END_ET),
            max_laps: b.i32(o::MAX_LAPS),
            track_length: b.f64(o::LAP_DIST),
            num_vehicles: b.i32(o::NUM_VEHICLES),
            game_phase: b.u8(o::GAME_PHASE),
            in_realtime: b.bool(o::IN_REALTIME),
            dark_cloud: b.f64(o::DARK_CLOUD),
            raining: b.f64(o::RAINING),
            min_path_wetness: b.f64(o::MIN_PATH_WETNESS),
            max_path_wetness: b.f64(o::MAX_PATH_WETNESS),
            avg_path_wetness: b.f64(o::AVG_PATH_WETNESS),
            ambient_temp_c: b.f64(o::AMBIENT_TEMP),
            track_temp_c: b.f64(o::TRACK_TEMP),
            game_mode: b.u8(o::GAME_MODE),
            max_players: b.i32(o::MAX_PLAYERS),
            is_fixed_setup: b.bool(o::IS_FIXED_SETUP),
            track_grip_level: b.u8(o::TRACK_GRIP_LEVEL),
            track_limits_steps_per_penalty: b.u8(o::TRACK_LIMITS_STEPS_PER_PENALTY),
            track_limits_steps_per_point: b.u8(o::TRACK_LIMITS_STEPS_PER_POINT),
            player: VehicleScoring::parse(&p),
        })
    }
}

/// Reads `mID` of a `TelemInfoV01` or `VehicleScoringInfoV01` (both start
/// with it).
#[cfg_attr(not(windows), allow(dead_code))] // used by the live source
pub(crate) fn slot_id(bytes: &[u8]) -> i32 {
    Bytes(bytes).i32(0)
}

/// `ScoringInfoV01.mNumVehicles`.
#[cfg_attr(not(windows), allow(dead_code))] // used by the live source
pub(crate) fn num_vehicles(info: &[u8]) -> i32 {
    Bytes(info).i32(off::scoring::NUM_VEHICLES)
}

/// `ScoringInfoV01.mInRealtime`.
#[cfg_attr(not(windows), allow(dead_code))] // used by the live source
pub(crate) fn in_realtime(info: &[u8]) -> bool {
    Bytes(info).bool(off::scoring::IN_REALTIME)
}

/// `TelemInfoV01.mElapsedTime`.
#[cfg_attr(not(windows), allow(dead_code))] // used by the live source
pub(crate) fn elapsed_time(telem: &[u8]) -> f64 {
    Bytes(telem).f64(off::telem::ELAPSED_TIME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoring_record_drops_what_isnt_needed() {
        let mut info = vec![0u8; SCORING_INFO_SIZE];
        let mut veh = vec![0u8; VEH_SCORING_SIZE];
        {
            let mut i = BytesMut(&mut info);
            i.str(off::scoring::TRACK_NAME, 64, "Circuit de Spa-Francorchamps");
            i.str(off::scoring::PLAYER_NAME, 32, "Some Driver");
            i.str(off::scoring::SERVER_NAME, 32, "My Server");
            i.put(off::scoring::SERVER_PUBLIC_IP, &[10, 0, 0, 1]);
            i.put(off::scoring::VEHICLE, &[0xAB; 8]);
            i.f64(off::scoring::LAP_DIST, 7004.0);
        }
        {
            let mut v = BytesMut(&mut veh);
            v.str(off::veh::DRIVER_NAME, 32, "Some Driver");
            v.put(off::veh::STEAM_ID, &7_656_119_800_000_000u64.to_le_bytes());
            v.u8(off::veh::IS_PLAYER, 1);
            v.i16(off::veh::TOTAL_LAPS, 3);
        }
        let rec = scoring_record(1_000, 42, &info, &veh);
        assert_eq!(rec.kind, KIND_SCORING);
        let s = Scoring::parse(&rec.payload).unwrap();
        assert_eq!(s.received_unix_ms, 1_000);
        assert_eq!(s.game_version, 42);
        assert_eq!(s.track_name, "Circuit de Spa-Francorchamps");
        assert_eq!(s.track_length, 7004.0);
        assert_eq!(s.player.total_laps, 3);
        assert!(s.player.is_player);
        let text = String::from_utf8_lossy(&rec.payload);
        assert!(!text.contains("My Server"));
        // the player's own name stays in the player's scoring only
        assert_eq!(text.matches("Some Driver").count(), 1);
        let info_part = &rec.payload[SCORING_HEADER..SCORING_HEADER + SCORING_INFO_SIZE];
        assert_eq!(Bytes(info_part).i32(off::scoring::SERVER_PUBLIC_IP), 0);
        assert_eq!(Bytes(info_part).f64(off::scoring::VEHICLE), 0.0);
        let veh_part = &rec.payload[SCORING_HEADER + SCORING_INFO_SIZE..];
        assert_eq!(Bytes(veh_part).f64(off::veh::STEAM_ID), 0.0);
    }

    #[test]
    fn wrong_sizes_are_rejected() {
        assert!(Telemetry::parse(&[0; 100]).is_err());
        assert!(Scoring::parse(&[0; 100]).is_err());
    }
}
