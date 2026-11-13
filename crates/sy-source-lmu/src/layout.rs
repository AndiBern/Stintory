// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Layout of LMU's shared memory `LMU_Data`.
//!
//! Source: Studio 397's `SharedMemoryInterface.hpp` and `InternalsPlugin.hpp`
//! from `<LMU>\Support\SharedMemoryInterface` (© 2025 Studio 397 BV and
//! Motorsport Games Inc., version shipped with LMU in March 2026). The
//! headers are not in the repository; these definitions are our own, written
//! from them (docs/PLAN.md §12.2).
//!
//! The structs below are never built or read directly: they only let the
//! compiler compute offsets and sizes (`offset_of!`, `size_of`) exactly the
//! way MSVC lays out the C++ structs for Windows x64:
//!
//! - `InternalsPlugin.hpp` sets `#pragma pack(4)`: `TelemInfoV01`,
//!   `VehicleScoringInfoV01`, `ScoringInfoV01`, `ApplicationStateV01` and
//!   their members are `repr(C, packed(4))` here.
//! - `SharedMemoryInterface.hpp` uses the default packing (8): the
//!   `SharedMemory*` structs are `repr(C)`.
//! - Windows x64 types: `long`/`unsigned long` = 4 bytes, pointers, `HWND`
//!   and `size_t` = 8 bytes, `bool` = 1 byte, `MAX_PATH` = 260.
//!
//! The sizes are asserted at compile time. `TelemInfoV01` (1888 bytes),
//! `TelemWheelV01` (260), `VehicleScoringInfoV01` (584) and
//! `ScoringInfoV01` (548) have the same sizes as in rFactor 2, whose
//! structs LMU extended inside their reserved space.

#![allow(dead_code)] // the structs exist for `offset_of!` and `size_of` only

use std::mem::{offset_of, size_of};

/// `TelemVect3`: three doubles.
#[repr(C, packed(4))]
pub(crate) struct Vect3 {
    x: f64,
    y: f64,
    z: f64,
}

/// `TelemWheelV01`
#[repr(C, packed(4))]
pub(crate) struct Wheel {
    pub suspension_deflection: f64, // m
    pub ride_height: f64,           // m
    pub susp_force: f64,            // N (pushrod load)
    pub brake_temp: f64,            // °C
    pub brake_pressure: f64,        // 0–1 (driver input and brake balance)
    pub rotation: f64,              // rad/s
    pub lateral_patch_vel: f64,
    pub longitudinal_patch_vel: f64,
    pub lateral_ground_vel: f64,
    pub longitudinal_ground_vel: f64,
    pub camber: f64, // rad
    pub lateral_force: f64,
    pub longitudinal_force: f64,
    pub tire_load: f64,        // N
    pub grip_fract: f64,       // fraction of the contact patch that is sliding
    pub pressure: f64,         // kPa
    pub temperature: [f64; 3], // K, left/centre/right
    pub wear: f64,             // 0–1 fraction of maximum
    pub terrain_name: [u8; 16],
    pub surface_type: u8, // 0 dry, 1 wet, 2 grass, 3 dirt, 4 gravel, 5 rumble strip, 6 special
    pub flat: u8,
    pub detached: u8,
    pub static_undeflected_radius: u8, // cm
    pub vertical_tire_deflection: f64,
    pub wheel_y_location: f64,
    pub toe: f64,
    pub tire_carcass_temperature: f64,          // K
    pub tire_inner_layer_temperature: [f64; 3], // K
    pub optimal_temp: f32,
    pub compound_index: u8,
    pub compound_type: u8,
    pub expansion: [u8; 18],
}

/// `TelemInfoV01`: one car's telemetry.
#[repr(C, packed(4))]
pub(crate) struct Telem {
    pub id: i32,
    pub delta_time: f64,
    pub elapsed_time: f64,
    pub lap_number: i32,
    pub lap_start_et: f64,
    pub vehicle_name: [u8; 64],
    pub track_name: [u8; 64],
    pub pos: Vect3,
    pub local_vel: Vect3,
    pub local_accel: Vect3,
    pub ori: [Vect3; 3],
    pub local_rot: Vect3,
    pub local_rot_accel: Vect3,
    pub gear: i32,
    pub engine_rpm: f64,
    pub engine_water_temp: f64,
    pub engine_oil_temp: f64,
    pub clutch_rpm: f64,
    pub unfiltered_throttle: f64,
    pub unfiltered_brake: f64,
    pub unfiltered_steering: f64,
    pub unfiltered_clutch: f64,
    pub filtered_throttle: f64,
    pub filtered_brake: f64,
    pub filtered_steering: f64,
    pub filtered_clutch: f64,
    pub steering_shaft_torque: f64,
    pub front_3rd_deflection: f64,
    pub rear_3rd_deflection: f64,
    pub front_wing_height: f64,
    pub front_ride_height: f64,
    pub rear_ride_height: f64,
    pub drag: f64,
    pub front_downforce: f64,
    pub rear_downforce: f64,
    pub fuel: f64, // l
    pub engine_max_rpm: f64,
    pub scheduled_stops: u8,
    pub overheating: u8,
    pub detached: u8,
    pub headlights: u8,
    pub dent_severity: [u8; 8],
    pub last_impact_et: f64,
    pub last_impact_magnitude: f64,
    pub last_impact_pos: Vect3,
    pub engine_torque: f64,
    /// zero-based sector, the pit lane in the sign bit
    pub current_sector: i32,
    pub speed_limiter: u8,
    pub max_gears: u8,
    pub front_tire_compound_index: u8,
    pub rear_tire_compound_index: u8,
    pub fuel_capacity: f64,
    pub front_flap_activated: u8,
    pub rear_flap_activated: u8,
    pub rear_flap_legal_status: u8,
    pub ignition_starter: u8,
    pub front_tire_compound_name: [u8; 18],
    pub rear_tire_compound_name: [u8; 18],
    pub speed_limiter_available: u8,
    pub anti_stall_activated: u8,
    pub unused: [u8; 2],
    pub visual_steering_wheel_range: f32,
    pub rear_brake_bias: f64,
    pub turbo_boost_pressure: f64,
    pub physics_to_graphics_offset: [f32; 3],
    pub physical_steering_wheel_range: f32,
    pub delta_best: f64,
    pub battery_charge_fraction: f64,
    pub electric_boost_motor_torque: f64,
    pub electric_boost_motor_rpm: f64,
    pub electric_boost_motor_temperature: f64,
    pub electric_boost_water_temperature: f64,
    pub electric_boost_motor_state: u8,
    pub lap_invalidated: u8,
    pub abs_active: u8,
    pub tc_active: u8,
    pub speed_limiter_active: u8,
    pub wiper_state: u8,
    pub tc: u8,
    pub tc_max: u8,
    pub tc_slip: u8,
    pub tc_slip_max: u8,
    pub tc_cut: u8,
    pub tc_cut_max: u8,
    pub abs: u8,
    pub abs_max: u8,
    pub motor_map: u8,
    pub motor_map_max: u8,
    pub migration: u8,
    pub migration_max: u8,
    pub front_anti_sway: u8,
    pub front_anti_sway_max: u8,
    pub rear_anti_sway: u8,
    pub rear_anti_sway_max: u8,
    pub lift_and_coast_progress: u8,
    pub track_limits_steps: u8,
    pub regen: f32, // kW
    pub soc: f32,
    pub virtual_energy: f32,
    pub time_gap_car_ahead: f32,
    pub time_gap_car_behind: f32,
    pub time_gap_place_ahead: f32,
    pub time_gap_place_behind: f32,
    pub vehicle_model: [u8; 30],
    pub vehicle_class: u8,
    pub vehicle_championship: u8,
    pub expansion: [u8; 20],
    /// front left, front right, rear left, rear right
    pub wheel: [Wheel; 4],
}

/// `VehicleScoringInfoV01`: one car's scoring.
#[repr(C, packed(4))]
pub(crate) struct VehScoring {
    pub id: i32,
    pub driver_name: [u8; 32],
    pub vehicle_name: [u8; 64],
    pub total_laps: i16,
    pub sector: i8, // 0 = sector 3, 1 = sector 1, 2 = sector 2
    pub finish_status: i8,
    pub lap_dist: f64,
    pub path_lateral: f64,
    pub track_edge: f64,
    pub best_sector1: f64,
    pub best_sector2: f64, // incl. sector 1
    pub best_lap_time: f64,
    pub last_sector1: f64,
    pub last_sector2: f64, // incl. sector 1
    pub last_lap_time: f64,
    pub cur_sector1: f64,
    pub cur_sector2: f64,
    pub num_pitstops: i16,
    pub num_penalties: i16,
    pub is_player: u8,
    pub control: i8,
    pub in_pits: u8,
    pub place: u8,
    pub vehicle_class: [u8; 32],
    pub time_behind_next: f64,
    pub laps_behind_next: i32,
    pub time_behind_leader: f64,
    pub laps_behind_leader: i32,
    pub lap_start_et: f64,
    pub pos: Vect3,
    pub local_vel: Vect3,
    pub local_accel: Vect3,
    pub ori: [Vect3; 3],
    pub local_rot: Vect3,
    pub local_rot_accel: Vect3,
    pub headlights: u8,
    pub pit_state: u8,
    pub server_scored: u8,
    pub individual_phase: u8,
    pub qualification: i32,
    pub time_into_lap: f64,
    pub estimated_lap_time: f64,
    pub pit_group: [u8; 24],
    pub flag: u8,
    pub under_yellow: u8,
    pub count_lap_flag: u8,
    pub in_garage_stall: u8,
    pub upgrade_pack: [u8; 16],
    pub pit_lap_dist: f32,
    pub best_lap_sector1: f32,
    pub best_lap_sector2: f32,
    pub steam_id: u64,
    pub veh_filename: [u8; 32],
    pub attack_mode: i16,
    pub fuel_fraction: u8,
    pub drs_state: u8,
    pub expansion: [u8; 4],
}

/// `ScoringInfoV01`: the session.
#[repr(C, packed(4))]
pub(crate) struct ScoringInfo {
    pub track_name: [u8; 64],
    /// 0 test day, 1–4 practice, 5–8 qualifying, 9 warm-up, 10–13 race
    pub session: i32,
    pub current_et: f64,
    pub end_et: f64,
    pub max_laps: i32,
    /// track length
    pub lap_dist: f64,
    pub results_stream: u64, // pointer
    pub num_vehicles: i32,
    pub game_phase: u8,
    pub yellow_flag_state: i8,
    pub sector_flag: [i8; 3],
    pub start_light: u8,
    pub num_red_lights: u8,
    pub in_realtime: u8,
    pub player_name: [u8; 32],
    pub plr_file_name: [u8; 64],
    pub dark_cloud: f64,
    pub raining: f64,
    pub ambient_temp: f64, // °C
    pub track_temp: f64,   // °C
    pub wind: Vect3,
    pub min_path_wetness: f64,
    pub max_path_wetness: f64,
    pub game_mode: u8, // 1 server, 2 client, 3 both
    pub is_password_protected: u8,
    pub server_port: u16,
    pub server_public_ip: u32,
    pub max_players: i32,
    pub server_name: [u8; 32],
    pub start_et: f32,
    pub avg_path_wetness: f64,
    pub session_time_remaining: f32,
    pub time_of_day: f32,
    pub is_fixed_setup: u8,
    pub track_grip_level: u8,
    pub cloud_coverage: u8,
    pub track_limits_steps_per_penalty: u8,
    pub track_limits_steps_per_point: u8,
    pub expansion: [u8; 187],
    pub vehicle: u64, // pointer
}

/// `ApplicationStateV01`
#[repr(C, packed(4))]
pub(crate) struct ApplicationState {
    pub app_window: u64, // HWND
    pub width: u32,
    pub height: u32,
    pub refresh_rate: u32,
    pub windowed: u32,
    pub options_location: u8, // 0 main UI, 1 track loading, 2 monitor, 3 on track
    pub options_page: [u8; 31],
    pub expansion: [u8; 204],
}

/// Number of entries of the `SharedMemoryEvent` enum (`SME_MAX`).
pub(crate) const EVENT_COUNT: usize = 16;
/// Number of cars in the arrays.
pub(crate) const MAX_VEHICLES: usize = 104;
/// `MAX_PATH`
const MAX_PATH: usize = 260;

/// `SharedMemoryGeneric`
#[repr(C)]
pub(crate) struct Generic {
    /// queue of `SharedMemoryEvent`s
    pub events: [u32; EVENT_COUNT],
    pub game_version: i32,
    pub ffb_torque: f32,
    pub app_info: ApplicationState,
}

/// `SharedMemoryPathData`
#[repr(C)]
pub(crate) struct Paths {
    pub user_data: [u8; MAX_PATH],
    pub custom_variables: [u8; MAX_PATH],
    pub steward_results: [u8; MAX_PATH],
    pub player_profile: [u8; MAX_PATH],
    pub plugins_folder: [u8; MAX_PATH],
}

/// `SharedMemoryScoringData`
#[repr(C)]
pub(crate) struct ScoringData {
    pub scoring_info: ScoringInfo,
    pub scoring_stream_size: u64, // size_t
    pub veh_scoring_info: [VehScoring; MAX_VEHICLES],
    pub scoring_stream: [u8; 65536],
}

/// `SharedMemoryTelemetryData`
#[repr(C)]
pub(crate) struct TelemetryData {
    pub active_vehicles: u8,
    pub player_vehicle_idx: u8,
    pub player_has_vehicle: u8,
    pub telem_info: [Telem; MAX_VEHICLES],
}

/// `SharedMemoryObjectOut`: the whole mapping.
#[repr(C)]
pub(crate) struct ObjectOut {
    pub generic: Generic,
    pub paths: Paths,
    pub scoring: ScoringData,
    pub telemetry: TelemetryData,
}

/// Size of `TelemInfoV01`.
pub const TELEM_SIZE: usize = size_of::<Telem>();
/// Size of `TelemWheelV01`.
pub const WHEEL_SIZE: usize = size_of::<Wheel>();
/// Size of `VehicleScoringInfoV01`.
pub const VEH_SCORING_SIZE: usize = size_of::<VehScoring>();
/// Size of `ScoringInfoV01`.
pub const SCORING_INFO_SIZE: usize = size_of::<ScoringInfo>();
/// Size of `SharedMemoryGeneric`.
pub const GENERIC_SIZE: usize = size_of::<Generic>();
/// Size of the whole mapping (`SharedMemoryObjectOut`).
pub const OBJECT_SIZE: usize = size_of::<ObjectOut>();

/// Offset of `generic.gameVersion` in the mapping.
pub const OFF_GAME_VERSION: usize = offset_of!(ObjectOut, generic.game_version);
/// Offset of `scoring.scoringInfo` in the mapping.
pub const OFF_SCORING_INFO: usize = offset_of!(ObjectOut, scoring.scoring_info);
/// Offset of `scoring.vehScoringInfo[0]` in the mapping.
pub const OFF_VEH_SCORING: usize = offset_of!(ObjectOut, scoring.veh_scoring_info);
/// Offset of `telemetry.activeVehicles` (followed by `playerVehicleIdx` and
/// `playerHasVehicle`) in the mapping.
pub const OFF_TELEMETRY: usize = offset_of!(ObjectOut, telemetry);
/// Offset of `telemetry.telemInfo[0]` in the mapping.
pub const OFF_TELEM_INFO: usize = offset_of!(ObjectOut, telemetry.telem_info);

// Sizes as computed by hand from the headers (see the module docs).
const _: () = assert!(WHEEL_SIZE == 260);
const _: () = assert!(TELEM_SIZE == 1888);
const _: () = assert!(VEH_SCORING_SIZE == 584);
const _: () = assert!(SCORING_INFO_SIZE == 548);
const _: () = assert!(size_of::<ApplicationState>() == 260);
const _: () = assert!(GENERIC_SIZE == 332);
const _: () = assert!(OFF_SCORING_INFO == 1632);
const _: () = assert!(OFF_VEH_SCORING == 2192);
const _: () = assert!(OFF_TELEMETRY == 128_464);
const _: () = assert!(OFF_TELEM_INFO == 128_468);
const _: () = assert!(OBJECT_SIZE == 324_824);

/// Field offsets inside the structs, short names for the parsers.
pub(crate) mod off {
    use super::{ScoringInfo, Telem, VehScoring, Wheel};
    use std::mem::offset_of;

    macro_rules! offsets {
        ($ty:ident: $($name:ident = $($field:ident).+;)*) => {
            $(pub const $name: usize = offset_of!($ty, $($field).+);)*
        };
    }

    /// `TelemInfoV01`
    pub mod telem {
        use super::*;
        offsets! { Telem:
            ID = id; DELTA_TIME = delta_time; ELAPSED_TIME = elapsed_time;
            LAP_NUMBER = lap_number; LAP_START_ET = lap_start_et;
            VEHICLE_NAME = vehicle_name; TRACK_NAME = track_name;
            POS = pos; LOCAL_VEL = local_vel; LOCAL_ACCEL = local_accel; ORI = ori;
            LOCAL_ROT = local_rot; GEAR = gear; ENGINE_RPM = engine_rpm;
            WATER_TEMP = engine_water_temp; OIL_TEMP = engine_oil_temp;
            THROTTLE = unfiltered_throttle; BRAKE = unfiltered_brake;
            STEERING = unfiltered_steering; CLUTCH = unfiltered_clutch;
            FRONT_RIDE_HEIGHT = front_ride_height; REAR_RIDE_HEIGHT = rear_ride_height;
            DRAG = drag; FRONT_DOWNFORCE = front_downforce; REAR_DOWNFORCE = rear_downforce;
            FUEL = fuel; ENGINE_MAX_RPM = engine_max_rpm; ENGINE_TORQUE = engine_torque;
            CURRENT_SECTOR = current_sector; SPEED_LIMITER = speed_limiter;
            MAX_GEARS = max_gears; FUEL_CAPACITY = fuel_capacity;
            FRONT_COMPOUND = front_tire_compound_name; REAR_COMPOUND = rear_tire_compound_name;
            FRONT_COMPOUND_INDEX = front_tire_compound_index;
            REAR_COMPOUND_INDEX = rear_tire_compound_index;
            REAR_BRAKE_BIAS = rear_brake_bias; TURBO_BOOST = turbo_boost_pressure;
            STEERING_RANGE = physical_steering_wheel_range;
            BATTERY = battery_charge_fraction;
            MOTOR_TORQUE = electric_boost_motor_torque; MOTOR_RPM = electric_boost_motor_rpm;
            MOTOR_STATE = electric_boost_motor_state;
            LAP_INVALIDATED = lap_invalidated; ABS_ACTIVE = abs_active; TC_ACTIVE = tc_active;
            TC = tc; TC_SLIP = tc_slip; TC_CUT = tc_cut; ABS = abs; MOTOR_MAP = motor_map;
            MIGRATION = migration; FRONT_ARB = front_anti_sway; REAR_ARB = rear_anti_sway;
            LIFT_AND_COAST = lift_and_coast_progress; TRACK_LIMITS_STEPS = track_limits_steps;
            REGEN = regen; VIRTUAL_ENERGY = virtual_energy;
            VEHICLE_MODEL = vehicle_model; VEHICLE_CLASS = vehicle_class;
            VEHICLE_CHAMPIONSHIP = vehicle_championship; WHEEL = wheel;
        }
    }

    /// `TelemWheelV01`
    pub mod wheel {
        use super::*;
        offsets! { Wheel:
            SUSPENSION_DEFLECTION = suspension_deflection; RIDE_HEIGHT = ride_height;
            BRAKE_TEMP = brake_temp; PRESSURE = pressure; TEMPERATURE = temperature;
            WEAR = wear; SURFACE_TYPE = surface_type; FLAT = flat; DETACHED = detached;
            CARCASS_TEMP = tire_carcass_temperature;
        }
    }

    /// `VehicleScoringInfoV01`
    pub mod veh {
        use super::*;
        offsets! { VehScoring:
            ID = id; DRIVER_NAME = driver_name; VEHICLE_NAME = vehicle_name;
            TOTAL_LAPS = total_laps; LAP_DIST = lap_dist;
            LAST_SECTOR1 = last_sector1; LAST_SECTOR2 = last_sector2;
            LAST_LAP_TIME = last_lap_time; IS_PLAYER = is_player; CONTROL = control;
            IN_PITS = in_pits; PLACE = place; VEHICLE_CLASS = vehicle_class;
            TIME_BEHIND_NEXT = time_behind_next; LAPS_BEHIND_NEXT = laps_behind_next;
            TIME_BEHIND_LEADER = time_behind_leader; LAPS_BEHIND_LEADER = laps_behind_leader;
            LAP_START_ET = lap_start_et; PIT_STATE = pit_state;
            COUNT_LAP_FLAG = count_lap_flag; STEAM_ID = steam_id;
            VEH_FILENAME = veh_filename;
        }
    }

    /// `ScoringInfoV01`
    pub mod scoring {
        use super::*;
        offsets! { ScoringInfo:
            TRACK_NAME = track_name; SESSION = session; CURRENT_ET = current_et;
            END_ET = end_et; MAX_LAPS = max_laps; LAP_DIST = lap_dist;
            RESULTS_STREAM = results_stream; NUM_VEHICLES = num_vehicles;
            GAME_PHASE = game_phase; IN_REALTIME = in_realtime;
            PLAYER_NAME = player_name; PLR_FILE_NAME = plr_file_name;
            DARK_CLOUD = dark_cloud; RAINING = raining;
            MIN_PATH_WETNESS = min_path_wetness; MAX_PATH_WETNESS = max_path_wetness;
            AVG_PATH_WETNESS = avg_path_wetness;
            AMBIENT_TEMP = ambient_temp; TRACK_TEMP = track_temp;
            GAME_MODE = game_mode; SERVER_PORT = server_port;
            SERVER_PUBLIC_IP = server_public_ip; MAX_PLAYERS = max_players;
            SERVER_NAME = server_name;
            IS_FIXED_SETUP = is_fixed_setup; TRACK_GRIP_LEVEL = track_grip_level;
            TRACK_LIMITS_STEPS_PER_PENALTY = track_limits_steps_per_penalty;
            TRACK_LIMITS_STEPS_PER_POINT = track_limits_steps_per_point;
            VEHICLE = vehicle;
        }
    }
}

/// Little-endian reads at an offset. Out of range reads give 0 (the
/// parsers check the record size first).
pub(crate) struct Bytes<'a>(pub &'a [u8]);

impl Bytes<'_> {
    fn array<const N: usize>(&self, at: usize) -> [u8; N] {
        self.0
            .get(at..at + N)
            .and_then(|b| b.try_into().ok())
            .unwrap_or([0; N])
    }

    pub fn u8(&self, at: usize) -> u8 {
        self.0.get(at).copied().unwrap_or(0)
    }

    pub fn i8(&self, at: usize) -> i8 {
        self.u8(at) as i8
    }

    pub fn bool(&self, at: usize) -> bool {
        self.u8(at) != 0
    }

    pub fn i16(&self, at: usize) -> i16 {
        i16::from_le_bytes(self.array(at))
    }

    pub fn i32(&self, at: usize) -> i32 {
        i32::from_le_bytes(self.array(at))
    }

    pub fn f32(&self, at: usize) -> f32 {
        f32::from_le_bytes(self.array(at))
    }

    pub fn f64(&self, at: usize) -> f64 {
        f64::from_le_bytes(self.array(at))
    }

    pub fn vect3(&self, at: usize) -> [f64; 3] {
        [self.f64(at), self.f64(at + 8), self.f64(at + 16)]
    }

    /// NUL-terminated string (the game writes Latin-1/ASCII; other bytes
    /// are replaced).
    pub fn str(&self, at: usize, len: usize) -> String {
        let bytes = self.0.get(at..at + len).unwrap_or(&[]);
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        bytes[..end]
            .iter()
            .map(|&b| char::from(b))
            .collect::<String>()
            .trim()
            .to_owned()
    }
}

/// Writes into a buffer at offsets (tests, the simulator, filtering).
pub(crate) struct BytesMut<'a>(pub &'a mut [u8]);

impl BytesMut<'_> {
    pub fn put(&mut self, at: usize, bytes: &[u8]) {
        self.0[at..at + bytes.len()].copy_from_slice(bytes);
    }

    pub fn zero(&mut self, at: usize, len: usize) {
        self.0[at..at + len].fill(0);
    }

    pub fn str(&mut self, at: usize, len: usize, s: &str) {
        self.zero(at, len);
        let b = s.as_bytes();
        let n = b.len().min(len - 1);
        self.0[at..at + n].copy_from_slice(&b[..n]);
    }

    pub fn u8(&mut self, at: usize, v: u8) {
        self.0[at] = v;
    }

    pub fn i16(&mut self, at: usize, v: i16) {
        self.put(at, &v.to_le_bytes());
    }

    pub fn i32(&mut self, at: usize, v: i32) {
        self.put(at, &v.to_le_bytes());
    }

    pub fn f32(&mut self, at: usize, v: f32) {
        self.put(at, &v.to_le_bytes());
    }

    pub fn f64(&mut self, at: usize, v: f64) {
        self.put(at, &v.to_le_bytes());
    }

    pub fn vect3(&mut self, at: usize, v: [f64; 3]) {
        for (i, x) in v.into_iter().enumerate() {
            self.f64(at + 8 * i, x);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_match_the_header_by_hand() {
        // a few fields counted by hand from InternalsPlugin.hpp
        assert_eq!(off::telem::ELAPSED_TIME, 12);
        assert_eq!(off::telem::POS, 160);
        assert_eq!(off::telem::GEAR, 352);
        assert_eq!(off::telem::FUEL, 524);
        assert_eq!(off::telem::CURRENT_SECTOR, 600);
        assert_eq!(off::telem::REAR_BRAKE_BIAS, 664);
        assert_eq!(off::telem::LAP_INVALIDATED, 745);
        assert_eq!(off::telem::VIRTUAL_ENERGY, 776);
        assert_eq!(off::telem::VEHICLE_MODEL, 796);
        assert_eq!(off::telem::WHEEL, 848);
        assert_eq!(off::wheel::PRESSURE, 120);
        assert_eq!(off::wheel::CARCASS_TEMP, 204);
        assert_eq!(off::veh::LAP_DIST, 104);
        assert_eq!(off::veh::LAST_LAP_TIME, 168);
        assert_eq!(off::veh::IS_PLAYER, 196);
        assert_eq!(off::veh::STEAM_ID, 536);
        assert_eq!(off::scoring::LAP_DIST, 88);
        assert_eq!(off::scoring::IN_REALTIME, 115);
        assert_eq!(off::scoring::SERVER_PUBLIC_IP, 288);
        assert_eq!(off::scoring::VEHICLE, 540);
    }
}
