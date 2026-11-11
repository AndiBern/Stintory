// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The binary packets of the F1 25 UDP telemetry, as far as Stintory uses
//! them.
//!
//! Source: EA's "F1 25 UDP specification" (packet format 2025) and "F1 25:
//! 2026 Season Pack UDP specification" (packet format 2026). Our own struct
//! definitions, written from the field lists of those specs; every packet
//! parser is checked against the packet size the spec states
//! ([`packet_size`]). All values are little-endian and packed.
//!
//! Differences between the formats that matter here:
//!
//! | | 2025 | 2026 |
//! |---|---|---|
//! | cars per array | 22 | 24 |
//! | motion: g-forces | `float` | `int16` / 1000 |
//! | participants: driver, network and team id | `uint8` | `uint16` |
//! | telemetry: engine temperature | `uint16` | `uint8` |
//! | status: ERS harvest limit per lap | – | `float` |
//! | session: active aero and DRS zones, assists | – | appended |
//! | packet 16 (car telemetry 2) | – | yes |

use std::fmt;

/// Length of the packet header (both formats).
pub const HEADER_LEN: usize = 29;

/// Packet ids (header `packetId`).
pub mod id {
    /// Motion of all cars
    pub const MOTION: u8 = 0;
    /// Session info
    pub const SESSION: u8 = 1;
    /// Lap data of all cars
    pub const LAP_DATA: u8 = 2;
    /// Events (session start/end, flashback, …)
    pub const EVENT: u8 = 3;
    /// Participants (names, teams)
    pub const PARTICIPANTS: u8 = 4;
    /// Car setups
    pub const CAR_SETUPS: u8 = 5;
    /// Car telemetry
    pub const CAR_TELEMETRY: u8 = 6;
    /// Car status
    pub const CAR_STATUS: u8 = 7;
    /// Final classification
    pub const FINAL_CLASSIFICATION: u8 = 8;
    /// Multiplayer lobby
    pub const LOBBY_INFO: u8 = 9;
    /// Car damage
    pub const CAR_DAMAGE: u8 = 10;
    /// Lap and tyre history of one car
    pub const SESSION_HISTORY: u8 = 11;
    /// Tyre sets of one car
    pub const TYRE_SETS: u8 = 12;
    /// Extra motion data of the player's car
    pub const MOTION_EX: u8 = 13;
    /// Time trial best laps
    pub const TIME_TRIAL: u8 = 14;
    /// Positions of all cars per lap
    pub const LAP_POSITIONS: u8 = 15;
    /// Car telemetry 2 (2026 format only)
    pub const CAR_TELEMETRY_2: u8 = 16;
}

/// The UDP format the game sends (in-game setting "UDP Format").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// F1 25 as released (`packetFormat` 2025)
    F2025,
    /// F1 25 with the 2026 Season Pack (`packetFormat` 2026)
    F2026,
}

impl Format {
    /// The format of a `packetFormat` value.
    pub fn from_packet_format(v: u16) -> Option<Self> {
        match v {
            2025 => Some(Self::F2025),
            2026 => Some(Self::F2026),
            _ => None,
        }
    }

    /// The `packetFormat` value.
    pub fn packet_format(self) -> u16 {
        match self {
            Self::F2025 => 2025,
            Self::F2026 => 2026,
        }
    }

    /// Number of entries in per-car arrays.
    pub fn cars(self) -> usize {
        match self {
            Self::F2025 => 22,
            Self::F2026 => 24,
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.packet_format())
    }
}

/// Packet size stated by the spec, or `None` for packets the format
/// doesn't have.
pub fn packet_size(format: Format, packet_id: u8) -> Option<usize> {
    let size = match (format, packet_id) {
        (Format::F2025, id::MOTION) => 1349,
        (Format::F2025, id::SESSION) => 753,
        (Format::F2025, id::LAP_DATA) => 1285,
        (Format::F2025, id::PARTICIPANTS) => 1284,
        (Format::F2025, id::CAR_SETUPS) => 1133,
        (Format::F2025, id::CAR_TELEMETRY) => 1352,
        (Format::F2025, id::CAR_STATUS) => 1239,
        (Format::F2025, id::FINAL_CLASSIFICATION) => 1042,
        (Format::F2025, id::LOBBY_INFO) => 954,
        (Format::F2025, id::CAR_DAMAGE) => 1041,
        (Format::F2025, id::TIME_TRIAL) => 101,
        (Format::F2025, id::LAP_POSITIONS) => 1131,
        (Format::F2026, id::MOTION) => 1325,
        (Format::F2026, id::SESSION) => 926,
        (Format::F2026, id::LAP_DATA) => 1399,
        (Format::F2026, id::PARTICIPANTS) => 1470,
        (Format::F2026, id::CAR_SETUPS) => 1233,
        (Format::F2026, id::CAR_TELEMETRY) => 1448,
        (Format::F2026, id::CAR_STATUS) => 1445,
        (Format::F2026, id::FINAL_CLASSIFICATION) => 1134,
        (Format::F2026, id::LOBBY_INFO) => 1062,
        (Format::F2026, id::CAR_DAMAGE) => 1133,
        (Format::F2026, id::TIME_TRIAL) => 104,
        (Format::F2026, id::LAP_POSITIONS) => 1231,
        (Format::F2026, id::CAR_TELEMETRY_2) => 269,
        (_, id::EVENT) => 45,
        (_, id::SESSION_HISTORY) => 1460,
        (_, id::TYRE_SETS) => 231,
        (_, id::MOTION_EX) => 273,
        _ => return None,
    };
    Some(size)
}

/// Per-car arrays of a packet: offset of the first entry and entry size.
pub fn car_array(format: Format, packet_id: u8) -> Option<(usize, usize)> {
    let new = format == Format::F2026;
    Some(match packet_id {
        id::MOTION => (HEADER_LEN, if new { 54 } else { 60 }),
        id::LAP_DATA => (HEADER_LEN, 57),
        id::PARTICIPANTS => (HEADER_LEN + 1, if new { 60 } else { 57 }),
        id::CAR_SETUPS => (HEADER_LEN, 50),
        id::CAR_TELEMETRY => (HEADER_LEN, if new { 59 } else { 60 }),
        id::CAR_STATUS => (HEADER_LEN, if new { 59 } else { 55 }),
        id::FINAL_CLASSIFICATION => (HEADER_LEN + 1, 46),
        id::LOBBY_INFO => (HEADER_LEN + 1, if new { 43 } else { 42 }),
        id::CAR_DAMAGE => (HEADER_LEN, 46),
        id::CAR_TELEMETRY_2 if new => (HEADER_LEN, 10),
        _ => return None,
    })
}

/// A packet couldn't be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PacketError {
    /// Not an F1 25 packet (unknown `packetFormat`)
    #[error("unknown packet format {0}")]
    UnknownFormat(u16),
    /// Unknown packet id for the format
    #[error("unknown packet id {id} in format {format}")]
    UnknownPacket {
        /// Format
        format: Format,
        /// Packet id
        id: u8,
    },
    /// Size differs from the spec
    #[error("packet {id} has {actual} bytes, expected {expected}")]
    WrongSize {
        /// Packet id
        id: u8,
        /// Size from the spec
        expected: usize,
        /// Received size
        actual: usize,
    },
    /// The player's car index is not a car
    #[error("player car index {0} out of range")]
    BadPlayerIndex(u8),
}

/// Little-endian cursor over a packet.
#[derive(Debug, Clone)]
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(buf: &'a [u8], pos: usize) -> Self {
        Self { buf, pos }
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    fn take<const N: usize>(&mut self) -> [u8; N] {
        // Packets are size-checked before parsing; a short read is a bug.
        let bytes: [u8; N] = self.buf[self.pos..self.pos + N]
            .try_into()
            .expect("size-checked packet");
        self.pos += N;
        bytes
    }

    pub(crate) fn skip(&mut self, n: usize) {
        self.pos += n;
    }

    pub(crate) fn u8(&mut self) -> u8 {
        self.take::<1>()[0]
    }

    pub(crate) fn i8(&mut self) -> i8 {
        i8::from_le_bytes(self.take())
    }

    pub(crate) fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.take())
    }

    pub(crate) fn i16(&mut self) -> i16 {
        i16::from_le_bytes(self.take())
    }

    pub(crate) fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take())
    }

    pub(crate) fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.take())
    }

    pub(crate) fn f32(&mut self) -> f32 {
        f32::from_le_bytes(self.take())
    }

    fn f32x4(&mut self) -> [f32; 4] {
        [self.f32(), self.f32(), self.f32(), self.f32()]
    }

    fn u8x4(&mut self) -> [u8; 4] {
        self.take()
    }

    fn u16x4(&mut self) -> [u16; 4] {
        [self.u16(), self.u16(), self.u16(), self.u16()]
    }
}

/// The header of every packet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Header {
    /// UDP format
    pub format: Format,
    /// Last two digits of the game year (stays 25 with the 2026 format)
    pub game_year: u8,
    /// Game major version
    pub game_major: u8,
    /// Game minor version
    pub game_minor: u8,
    /// Version of this packet type
    pub packet_version: u8,
    /// Packet id (see [`id`])
    pub packet_id: u8,
    /// Unique id of the session
    pub session_uid: u64,
    /// Session time, seconds
    pub session_time: f32,
    /// Frame the data was retrieved on (goes back after a flashback)
    pub frame: u32,
    /// Frame counter that doesn't go back after a flashback
    pub overall_frame: u32,
    /// Index of the player's car in per-car arrays
    pub player_car: u8,
    /// Index of the second player's car (split screen), 255 if none
    pub secondary_player_car: u8,
}

impl Header {
    /// Reads and checks the header: known format, known packet id, the
    /// size the spec states, a valid player index.
    pub fn parse(packet: &[u8]) -> Result<Self, PacketError> {
        if packet.len() < HEADER_LEN {
            return Err(PacketError::WrongSize {
                id: packet.get(6).copied().unwrap_or(255),
                expected: HEADER_LEN,
                actual: packet.len(),
            });
        }
        let mut r = Reader::new(packet, 0);
        let raw_format = r.u16();
        let format =
            Format::from_packet_format(raw_format).ok_or(PacketError::UnknownFormat(raw_format))?;
        let h = Self {
            format,
            game_year: r.u8(),
            game_major: r.u8(),
            game_minor: r.u8(),
            packet_version: r.u8(),
            packet_id: r.u8(),
            session_uid: r.u64(),
            session_time: r.f32(),
            frame: r.u32(),
            overall_frame: r.u32(),
            player_car: r.u8(),
            secondary_player_car: r.u8(),
        };
        debug_assert_eq!(r.pos(), HEADER_LEN);
        let expected = packet_size(format, h.packet_id).ok_or(PacketError::UnknownPacket {
            format,
            id: h.packet_id,
        })?;
        if packet.len() != expected {
            return Err(PacketError::WrongSize {
                id: h.packet_id,
                expected,
                actual: packet.len(),
            });
        }
        if usize::from(h.player_car) >= format.cars() {
            return Err(PacketError::BadPlayerIndex(h.player_car));
        }
        Ok(h)
    }

    /// Reader at the player's entry of a per-car array.
    fn player_entry<'a>(&self, packet: &'a [u8]) -> Reader<'a> {
        let (offset, stride) =
            car_array(self.format, self.packet_id).expect("packet with a car array");
        Reader::new(packet, offset + stride * usize::from(self.player_car))
    }
}

/// Time given as milliseconds part + minutes part.
fn split_ms(ms: u16, minutes: u8) -> u32 {
    u32::from(ms) + u32::from(minutes) * 60_000
}

/// Motion of the player's car (packet 0).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CarMotion {
    /// World position, metres (Y is up)
    pub position: [f32; 3],
    /// World velocity, m/s
    pub velocity: [f32; 3],
    /// Lateral, longitudinal, vertical g-force, in g
    pub g_force: [f32; 3],
    /// Yaw, pitch, roll, radians
    pub yaw_pitch_roll: [f32; 3],
}

impl CarMotion {
    /// The player's entry.
    pub fn parse(h: &Header, packet: &[u8]) -> Self {
        let mut r = h.player_entry(packet);
        let position = [r.f32(), r.f32(), r.f32()];
        let velocity = [r.f32(), r.f32(), r.f32()];
        r.skip(12); // forward and right direction, int16 x 6
        let g_force = match h.format {
            Format::F2025 => [r.f32(), r.f32(), r.f32()],
            Format::F2026 => [
                f32::from(r.i16()) / 1000.0,
                f32::from(r.i16()) / 1000.0,
                f32::from(r.i16()) / 1000.0,
            ],
        };
        let yaw_pitch_roll = [r.f32(), r.f32(), r.f32()];
        Self {
            position,
            velocity,
            g_force,
            yaw_pitch_roll,
        }
    }
}

/// Session info (packet 1), the fields Stintory uses.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    /// Weather (see `tables::weather`)
    pub weather: u8,
    /// Track temperature, °C
    pub track_temp_c: i8,
    /// Air temperature, °C
    pub air_temp_c: i8,
    /// Laps in the race
    pub total_laps: u8,
    /// Track length, metres
    pub track_length_m: u16,
    /// Session type (see `tables::session_type`)
    pub session_type: u8,
    /// Track id, -1 unknown
    pub track_id: i8,
    /// Formula (0 F1 Modern, 1 F1 Classic, 2 F2, 3 F1 Generic, …)
    pub formula: u8,
    /// Game paused
    pub paused: bool,
    /// The player is spectating (the data is about another car)
    pub spectating: bool,
    /// Online game
    pub network_game: bool,
    /// AI difficulty 0–110
    pub ai_difficulty: u8,
    /// Identifier of the session across packets of a weekend
    pub session_link: u32,
    /// Assists: steering, braking, gearbox, pit, pit release, ERS, DRS,
    /// racing line, racing line type
    pub assists: [u8; 9],
    /// Game mode (see the spec's appendix)
    pub game_mode: u8,
    /// Rule set (see the spec's appendix)
    pub rule_set: u8,
    /// 0 none, 1 flashbacks, 2 auto-recovery
    pub recovery_mode: u8,
    /// Distance where sector 2 starts, metres
    pub sector2_start_m: f32,
    /// Distance where sector 3 starts, metres
    pub sector3_start_m: f32,
}

impl Session {
    /// Parses the packet.
    pub fn parse(h: &Header, packet: &[u8]) -> Self {
        let mut r = Reader::new(packet, HEADER_LEN);
        let weather = r.u8();
        let track_temp_c = r.i8();
        let air_temp_c = r.i8();
        let total_laps = r.u8();
        let track_length_m = r.u16();
        let session_type = r.u8();
        let track_id = r.i8();
        let formula = r.u8();
        r.skip(2 + 2 + 1); // time left, duration, pit speed limit
        let paused = r.u8() != 0;
        let spectating = r.u8() != 0;
        r.skip(1 + 1); // spectator car index, SLI Pro support
        r.skip(1 + 21 * 5); // marshal zones
        r.skip(1); // safety car status
        let network_game = r.u8() != 0;
        r.skip(1 + 64 * 8 + 1); // weather forecast, forecast accuracy
        let ai_difficulty = r.u8();
        r.skip(4 + 4); // season and weekend link
        let session_link = r.u32();
        r.skip(3); // pit stop window, rejoin position
        let mut assists = [0; 9];
        for a in &mut assists {
            *a = r.u8();
        }
        let game_mode = r.u8();
        let rule_set = r.u8();
        r.skip(4 + 1 + 4 + 3); // time of day, session length, units, SC/VSC/red flag counts
        r.skip(1); // equal car performance
        let recovery_mode = r.u8();
        r.skip(23 + 12); // further rule settings, weekend structure
        let sector2_start_m = r.f32();
        let sector3_start_m = r.f32();
        debug_assert_eq!(r.pos(), 753);
        if h.format == Format::F2026 {
            // active aero zones, DRS zones, start reaction time, assists
            r.skip(1 + 1 + 64 + 1 + 64 + 1 + 32 + 4 + 5);
            debug_assert_eq!(r.pos(), 926);
        }
        Self {
            weather,
            track_temp_c,
            air_temp_c,
            total_laps,
            track_length_m,
            session_type,
            track_id,
            formula,
            paused,
            spectating,
            network_game,
            ai_difficulty,
            session_link,
            assists,
            game_mode,
            rule_set,
            recovery_mode,
            sector2_start_m,
            sector3_start_m,
        }
    }
}

/// Lap data of the player's car (packet 2).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LapData {
    /// Time of the last completed lap, ms
    pub last_lap_ms: u32,
    /// Time in the current lap, ms
    pub current_lap_ms: u32,
    /// Sector 1 time of the current lap, ms (0 until done)
    pub sector1_ms: u32,
    /// Sector 2 time of the current lap, ms (0 until done)
    pub sector2_ms: u32,
    /// Distance in the lap, metres (negative before the line)
    pub lap_distance_m: f32,
    /// Total distance in the session, metres
    pub total_distance_m: f32,
    /// Race position
    pub position: u8,
    /// Lap being driven
    pub lap: u8,
    /// 0 none, 1 pitting, 2 in the pit area
    pub pit_status: u8,
    /// 0 sector 1, 1 sector 2, 2 sector 3
    pub sector: u8,
    /// The current lap is invalid
    pub lap_invalid: bool,
    /// Penalty seconds
    pub penalties_s: u8,
    /// Warnings in total
    pub warnings: u8,
    /// Corner cutting warnings
    pub corner_cutting_warnings: u8,
    /// 0 in garage, 1 flying lap, 2 in-lap, 3 out-lap, 4 on track
    pub driver_status: u8,
    /// 0 invalid, 1 inactive, 2 active, 3 finished, 4 DNF, 5 DSQ, 6 not
    /// classified, 7 retired
    pub result_status: u8,
}

impl LapData {
    /// The player's entry.
    pub fn parse(h: &Header, packet: &[u8]) -> Self {
        let mut r = h.player_entry(packet);
        let last_lap_ms = r.u32();
        let current_lap_ms = r.u32();
        let s1 = r.u16();
        let s1_min = r.u8();
        let s2 = r.u16();
        let s2_min = r.u8();
        r.skip(6); // deltas to the car in front and the leader
        let lap_distance_m = r.f32();
        let total_distance_m = r.f32();
        r.skip(4); // safety car delta
        let position = r.u8();
        let lap = r.u8();
        let pit_status = r.u8();
        r.skip(1); // number of pit stops
        let sector = r.u8();
        let lap_invalid = r.u8() != 0;
        let penalties_s = r.u8();
        let warnings = r.u8();
        let corner_cutting_warnings = r.u8();
        r.skip(3); // unserved penalties, grid position
        let driver_status = r.u8();
        let result_status = r.u8();
        Self {
            last_lap_ms,
            current_lap_ms,
            sector1_ms: split_ms(s1, s1_min),
            sector2_ms: split_ms(s2, s2_min),
            lap_distance_m,
            total_distance_m,
            position,
            lap,
            pit_status,
            sector,
            lap_invalid,
            penalties_s,
            warnings,
            corner_cutting_warnings,
            driver_status,
            result_status,
        }
    }
}

/// An event (packet 3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// `SSTA`: session started
    SessionStarted,
    /// `SEND`: session ended
    SessionEnded,
    /// `FLBK`: flashback to a frame / session time
    Flashback {
        /// Frame identifier flashed back to
        frame: u32,
        /// Session time flashed back to, seconds
        session_time: f32,
    },
    /// `CHQF`: chequered flag
    ChequeredFlag,
    /// `RDFL`: red flag
    RedFlag,
    /// `LGOT`: lights out
    LightsOut,
    /// `SCAR`: safety car (type 0 none, 1 full, 2 virtual, 3 formation
    /// lap; event 0 deployed, 1 returning, 2 returned, 3 resume race)
    SafetyCar {
        /// Safety car type
        kind: u8,
        /// What happened
        event: u8,
    },
    /// Any other event (they are about other cars or not needed)
    Other([u8; 4]),
}

impl Event {
    /// The event's 4-character code.
    pub fn code(packet: &[u8]) -> [u8; 4] {
        packet[HEADER_LEN..HEADER_LEN + 4]
            .try_into()
            .expect("size-checked packet")
    }

    /// Parses the packet.
    pub fn parse(packet: &[u8]) -> Self {
        let code = Self::code(packet);
        let mut r = Reader::new(packet, HEADER_LEN + 4);
        match &code {
            b"SSTA" => Self::SessionStarted,
            b"SEND" => Self::SessionEnded,
            b"FLBK" => Self::Flashback {
                frame: r.u32(),
                session_time: r.f32(),
            },
            b"CHQF" => Self::ChequeredFlag,
            b"RDFL" => Self::RedFlag,
            b"LGOT" => Self::LightsOut,
            b"SCAR" => Self::SafetyCar {
                kind: r.u8(),
                event: r.u8(),
            },
            _ => Self::Other(code),
        }
    }
}

/// The player's entry of the participants (packet 4).
#[derive(Debug, Clone, PartialEq)]
pub struct Participant {
    /// Controlled by the AI
    pub ai_controlled: bool,
    /// Team id (see the spec's appendix)
    pub team_id: u16,
    /// Race number
    pub race_number: u8,
    /// Telemetry setting: false = restricted
    pub telemetry_public: bool,
}

impl Participant {
    /// The player's entry.
    pub fn parse(h: &Header, packet: &[u8]) -> Self {
        let mut r = h.player_entry(packet);
        let ai_controlled = r.u8() != 0;
        let team_id = match h.format {
            Format::F2025 => {
                r.skip(2); // driver id, network id
                u16::from(r.u8())
            }
            Format::F2026 => {
                r.skip(4);
                r.u16()
            }
        };
        r.skip(1); // my team
        let race_number = r.u8();
        r.skip(1 + 32); // nationality, name
        let telemetry_public = r.u8() != 0;
        Self {
            ai_controlled,
            team_id,
            race_number,
            telemetry_public,
        }
    }
}

/// The player's car setup (packet 5).
#[derive(Debug, Clone, PartialEq)]
pub struct CarSetup {
    /// Front and rear wing
    pub wings: [u8; 2],
    /// Differential on/off throttle, %
    pub diff: [u8; 2],
    /// Front/rear camber, front/rear toe, degrees
    pub camber_toe: [f32; 4],
    /// Front/rear suspension, front/rear anti-roll bar, front/rear ride
    /// height, brake pressure %, brake bias %, engine braking %
    pub chassis: [u8; 9],
    /// Tyre pressures RL, RR, FL, FR, PSI
    pub tyre_pressures_psi: [f32; 4],
    /// Ballast
    pub ballast: u8,
    /// Fuel load
    pub fuel_load: f32,
}

impl CarSetup {
    /// The player's entry.
    pub fn parse(h: &Header, packet: &[u8]) -> Self {
        let mut r = h.player_entry(packet);
        let wings = [r.u8(), r.u8()];
        let diff = [r.u8(), r.u8()];
        let camber_toe = r.f32x4();
        let mut chassis = [0; 9];
        for c in &mut chassis {
            *c = r.u8();
        }
        let tyre_pressures_psi = r.f32x4();
        Self {
            wings,
            diff,
            camber_toe,
            chassis,
            tyre_pressures_psi,
            ballast: r.u8(),
            fuel_load: r.f32(),
        }
    }

    /// The setup as JSON (stored with the session, exported only on request).
    pub fn to_json(&self) -> serde_json::Value {
        let [rl, rr, fl, fr] = self.tyre_pressures_psi;
        let [fc, rc, ft, rt] = self.camber_toe;
        let c = self.chassis;
        serde_json::json!({
            "front_wing": self.wings[0], "rear_wing": self.wings[1],
            "diff_on_throttle": self.diff[0], "diff_off_throttle": self.diff[1],
            "front_camber": fc, "rear_camber": rc, "front_toe": ft, "rear_toe": rt,
            "front_suspension": c[0], "rear_suspension": c[1],
            "front_anti_roll_bar": c[2], "rear_anti_roll_bar": c[3],
            "front_ride_height": c[4], "rear_ride_height": c[5],
            "brake_pressure": c[6], "brake_bias": c[7], "engine_braking": c[8],
            "tyre_pressure_psi": { "rl": rl, "rr": rr, "fl": fl, "fr": fr },
            "ballast": self.ballast, "fuel_load": self.fuel_load,
        })
    }
}

/// Telemetry of the player's car (packet 6).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CarTelemetry {
    /// Speed, km/h
    pub speed_kmh: u16,
    /// Throttle 0–1
    pub throttle: f32,
    /// Steering -1 (full lock left) … 1 (full lock right)
    pub steer: f32,
    /// Brake 0–1
    pub brake: f32,
    /// Clutch 0–100
    pub clutch: u8,
    /// Gear: -1 reverse, 0 neutral, 1–8
    pub gear: i8,
    /// Engine speed, rpm
    pub rpm: u16,
    /// DRS open
    pub drs: bool,
    /// Brake temperatures RL, RR, FL, FR, °C
    pub brake_temp_c: [u16; 4],
    /// Tyre surface temperatures RL, RR, FL, FR, °C
    pub tyre_surface_temp_c: [u8; 4],
    /// Tyre inner temperatures RL, RR, FL, FR, °C
    pub tyre_inner_temp_c: [u8; 4],
    /// Engine temperature, °C
    pub engine_temp_c: u16,
    /// Tyre pressures RL, RR, FL, FR, PSI
    pub tyre_pressure_psi: [f32; 4],
    /// Surface under each wheel RL, RR, FL, FR (see the spec's appendix)
    pub surface: [u8; 4],
}

impl CarTelemetry {
    /// The player's entry.
    pub fn parse(h: &Header, packet: &[u8]) -> Self {
        let mut r = h.player_entry(packet);
        let speed_kmh = r.u16();
        let throttle = r.f32();
        let steer = r.f32();
        let brake = r.f32();
        let clutch = r.u8();
        let gear = r.i8();
        let rpm = r.u16();
        let drs = r.u8() != 0;
        r.skip(1 + 2); // rev lights
        let brake_temp_c = r.u16x4();
        let tyre_surface_temp_c = r.u8x4();
        let tyre_inner_temp_c = r.u8x4();
        let engine_temp_c = match h.format {
            Format::F2025 => r.u16(),
            Format::F2026 => u16::from(r.u8()),
        };
        Self {
            speed_kmh,
            throttle,
            steer,
            brake,
            clutch,
            gear,
            rpm,
            drs,
            brake_temp_c,
            tyre_surface_temp_c,
            tyre_inner_temp_c,
            engine_temp_c,
            tyre_pressure_psi: r.f32x4(),
            surface: r.u8x4(),
        }
    }
}

/// Status of the player's car (packet 7).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CarStatus {
    /// Traction control 0 off, 1 medium, 2 full
    pub traction_control: u8,
    /// ABS on
    pub abs: bool,
    /// Fuel mix 0 lean, 1 standard, 2 rich, 3 max
    pub fuel_mix: u8,
    /// Front brake bias, %
    pub front_brake_bias: u8,
    /// Pit limiter on
    pub pit_limiter: bool,
    /// Fuel in the tank, kg
    pub fuel_kg: f32,
    /// Engine speed limit, rpm
    pub max_rpm: u16,
    /// Actual tyre compound (see the spec)
    pub tyre_compound: u8,
    /// Visual tyre compound
    pub tyre_visual_compound: u8,
    /// Laps on this set of tyres
    pub tyre_age_laps: u8,
    /// ERS energy store, joules
    pub ers_store_j: f32,
    /// ERS deploy mode 0 none, 1 medium, 2 hotlap, 3 overtake
    pub ers_deploy_mode: u8,
    /// ERS deployed in this lap, joules
    pub ers_deployed_lap_j: f32,
}

impl CarStatus {
    /// The player's entry.
    pub fn parse(h: &Header, packet: &[u8]) -> Self {
        let mut r = h.player_entry(packet);
        let traction_control = r.u8();
        let abs = r.u8() != 0;
        let fuel_mix = r.u8();
        let front_brake_bias = r.u8();
        let pit_limiter = r.u8() != 0;
        let fuel_kg = r.f32();
        r.skip(4 + 4); // capacity, remaining laps
        let max_rpm = r.u16();
        r.skip(2 + 1 + 1 + 2); // idle rpm, max gears, DRS allowed, DRS distance
        let tyre_compound = r.u8();
        let tyre_visual_compound = r.u8();
        let tyre_age_laps = r.u8();
        r.skip(1 + 4 + 4); // FIA flag, ICE and MGU-K power
        let ers_store_j = r.f32();
        let ers_deploy_mode = r.u8();
        r.skip(4 + 4); // harvested MGU-K, MGU-H
        if h.format == Format::F2026 {
            r.skip(4); // harvest limit per lap
        }
        let ers_deployed_lap_j = r.f32();
        Self {
            traction_control,
            abs,
            fuel_mix,
            front_brake_bias,
            pit_limiter,
            fuel_kg,
            max_rpm,
            tyre_compound,
            tyre_visual_compound,
            tyre_age_laps,
            ers_store_j,
            ers_deploy_mode,
            ers_deployed_lap_j,
        }
    }
}

/// One lap of the session history (packet 11).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LapHistory {
    /// Lap time, ms (0 = not completed)
    pub lap_time_ms: u32,
    /// Sector times, ms
    pub sectors_ms: [u32; 3],
    /// Bits: 0x01 lap valid, 0x02/0x04/0x08 sector 1/2/3 valid
    pub valid_flags: u8,
}

impl LapHistory {
    /// The lap counts (bit 0x01).
    pub fn lap_valid(&self) -> bool {
        self.valid_flags & 0x01 != 0
    }
}

/// Lap history of one car (packet 11).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionHistory {
    /// The car this history is about
    pub car: u8,
    /// Laps, including the one being driven; index 0 = lap 1
    pub laps: Vec<LapHistory>,
}

impl SessionHistory {
    /// The car the packet is about (before parsing all of it).
    pub fn car(packet: &[u8]) -> u8 {
        packet[HEADER_LEN]
    }

    /// Parses the packet.
    pub fn parse(packet: &[u8]) -> Self {
        let mut r = Reader::new(packet, HEADER_LEN);
        let car = r.u8();
        let num_laps = usize::from(r.u8()).min(100);
        r.skip(5); // stints, best lap and sector lap numbers
        let mut laps = Vec::with_capacity(num_laps);
        for i in 0..100 {
            let lap_time_ms = r.u32();
            let s1 = split_ms(r.u16(), r.u8());
            let s2 = split_ms(r.u16(), r.u8());
            let s3 = split_ms(r.u16(), r.u8());
            let valid_flags = r.u8();
            if i < num_laps {
                laps.push(LapHistory {
                    lap_time_ms,
                    sectors_ms: [s1, s2, s3],
                    valid_flags,
                });
            }
        }
        r.skip(8 * 3); // tyre stints
        debug_assert_eq!(r.pos(), 1460);
        Self { car, laps }
    }
}

/// Extra motion data of the player's car (packet 13).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MotionEx {
    /// Suspension position RL, RR, FL, FR
    pub suspension_position: [f32; 4],
    /// Wheel speed RL, RR, FL, FR
    pub wheel_speed: [f32; 4],
    /// Wheel slip ratio RL, RR, FL, FR
    pub wheel_slip_ratio: [f32; 4],
    /// Wheel slip angle RL, RR, FL, FR
    pub wheel_slip_angle: [f32; 4],
    /// Velocity in the car's frame (x, y, z)
    pub local_velocity: [f32; 3],
    /// Angular velocity (x, y, z), rad/s
    pub angular_velocity: [f32; 3],
    /// Angle of the front wheels, radians
    pub front_wheels_angle: f32,
    /// Front and rear aero (ride) height
    pub aero_height: [f32; 2],
}

impl MotionEx {
    /// Parses the packet.
    pub fn parse(packet: &[u8]) -> Self {
        let mut r = Reader::new(packet, HEADER_LEN);
        let suspension_position = r.f32x4();
        r.skip(2 * 16); // suspension velocity, acceleration
        let wheel_speed = r.f32x4();
        let wheel_slip_ratio = r.f32x4();
        let wheel_slip_angle = r.f32x4();
        r.skip(2 * 16 + 4); // lateral/longitudinal wheel forces, COG height
        let local_velocity = [r.f32(), r.f32(), r.f32()];
        let angular_velocity = [r.f32(), r.f32(), r.f32()];
        r.skip(12); // angular acceleration
        let front_wheels_angle = r.f32();
        r.skip(16); // vertical wheel forces
        let aero_height = [r.f32(), r.f32()];
        r.skip(4 * 4 + 16 + 16); // roll angles, chassis yaw/pitch, camber, camber gain
        debug_assert_eq!(r.pos(), 273);
        Self {
            suspension_position,
            wheel_speed,
            wheel_slip_ratio,
            wheel_slip_angle,
            local_velocity,
            angular_velocity,
            front_wheels_angle,
            aero_height,
        }
    }
}

/// Car telemetry 2 of the player's car (packet 16, 2026 format).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CarTelemetry2 {
    /// Active aero: 0 corner mode, 1 straight mode
    pub active_aero_mode: u8,
    /// Active aero can be used
    pub active_aero_available: bool,
    /// Overtake mode can be used
    pub overtake_available: bool,
    /// Overtake mode is on
    pub overtake_active: bool,
    /// The car follows the 2026 regulations
    pub regulations_2026: bool,
    /// Driving the wrong way
    pub wrong_way: bool,
}

impl CarTelemetry2 {
    /// The player's entry.
    pub fn parse(h: &Header, packet: &[u8]) -> Self {
        let mut r = h.player_entry(packet);
        let active_aero_mode = r.u8();
        let active_aero_available = r.u8() != 0;
        r.skip(2);
        let overtake_available = r.u8() != 0;
        let overtake_active = r.u8() != 0;
        r.skip(2);
        Self {
            active_aero_mode,
            active_aero_available,
            overtake_available,
            overtake_active,
            regulations_2026: r.u8() != 0,
            wrong_way: r.u8() != 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::PacketBuilder;

    #[test]
    fn per_car_arrays_fill_the_stated_sizes() {
        for format in [Format::F2025, Format::F2026] {
            for id in 0..=16u8 {
                let Some((offset, stride)) = car_array(format, id) else {
                    continue;
                };
                let trailing = match id {
                    id::LAP_DATA => 2,
                    id::CAR_SETUPS => 4,
                    id::CAR_TELEMETRY => 3,
                    _ => 0,
                };
                assert_eq!(
                    offset + stride * format.cars() + trailing,
                    packet_size(format, id).unwrap(),
                    "format {format} packet {id}"
                );
            }
        }
    }

    #[test]
    fn header_checks_format_size_and_player() {
        let p = PacketBuilder::new(Format::F2026, id::LAP_DATA, 42, 12.5, 3);
        let h = Header::parse(&p.buf).unwrap();
        assert_eq!(h.format, Format::F2026);
        assert_eq!((h.session_uid, h.session_time, h.player_car), (42, 12.5, 3));
        assert_eq!(
            Header::parse(&p.buf[..100]),
            Err(PacketError::WrongSize {
                id: 2,
                expected: 1399,
                actual: 100
            })
        );
        let mut bad = p.buf.clone();
        bad[0..2].copy_from_slice(&2024u16.to_le_bytes());
        assert_eq!(Header::parse(&bad), Err(PacketError::UnknownFormat(2024)));
        let p16 = PacketBuilder::new(Format::F2026, id::CAR_TELEMETRY_2, 1, 0.0, 0);
        let mut old = p16.buf.clone();
        old[0..2].copy_from_slice(&2025u16.to_le_bytes());
        assert!(matches!(
            Header::parse(&old),
            Err(PacketError::UnknownPacket { id: 16, .. })
        ));
        let mut p = PacketBuilder::new(Format::F2025, id::LAP_DATA, 1, 0.0, 0);
        p.buf[27] = 22;
        assert_eq!(Header::parse(&p.buf), Err(PacketError::BadPlayerIndex(22)));
    }

    #[test]
    fn reads_the_players_entries() {
        for format in [Format::F2025, Format::F2026] {
            let mut p = PacketBuilder::new(format, id::CAR_TELEMETRY, 1, 0.0, 5);
            let at = p.car(5, 0);
            p.put(at, &250u16.to_le_bytes())
                .put(at + 2, &0.5f32.to_le_bytes())
                .put(at + 6, &(-0.25f32).to_le_bytes())
                .put(at + 14, &[40, 0xFF]) // clutch 40, gear -1
                .put(at + 16, &11_000u16.to_le_bytes())
                .put(at + 18, &[1]);
            // other cars' data is not read
            let other = p.car(4, 0);
            p.put(other, &99u16.to_le_bytes());
            let h = Header::parse(&p.buf).unwrap();
            let t = CarTelemetry::parse(&h, &p.buf);
            assert_eq!(t.speed_kmh, 250);
            assert_eq!(
                (t.throttle, t.steer, t.clutch, t.gear),
                (0.5, -0.25, 40, -1)
            );
            assert_eq!((t.rpm, t.drs), (11_000, true));
        }
    }

    #[test]
    fn g_forces_per_format() {
        let mut p = PacketBuilder::new(Format::F2026, id::MOTION, 1, 0.0, 0);
        let at = p.car(0, 36);
        p.put(at, &1500i16.to_le_bytes())
            .put(at + 2, &(-2000i16).to_le_bytes());
        let h = Header::parse(&p.buf).unwrap();
        assert_eq!(CarMotion::parse(&h, &p.buf).g_force, [1.5, -2.0, 0.0]);

        let mut p = PacketBuilder::new(Format::F2025, id::MOTION, 1, 0.0, 0);
        let at = p.car(0, 36);
        p.put(at, &1.5f32.to_le_bytes())
            .put(at + 4, &(-2.0f32).to_le_bytes());
        let at = p.car(0, 48);
        p.put(at, &0.75f32.to_le_bytes());
        let h = Header::parse(&p.buf).unwrap();
        let m = CarMotion::parse(&h, &p.buf);
        assert_eq!(m.g_force, [1.5, -2.0, 0.0]);
        assert_eq!(m.yaw_pitch_roll[0], 0.75);
    }

    #[test]
    fn lap_data_and_history() {
        let mut p = PacketBuilder::new(Format::F2025, id::LAP_DATA, 1, 0.0, 1);
        let at = p.car(1, 0);
        p.put(at, &83_456u32.to_le_bytes())
            .put(at + 8, &1_500u16.to_le_bytes())
            .put(at + 10, &[1])
            .put(at + 20, &1234.5f32.to_le_bytes())
            .put(at + 33, &[7, 2])
            .put(at + 37, &[1])
            .put(at + 44, &[3]);
        let h = Header::parse(&p.buf).unwrap();
        let l = LapData::parse(&h, &p.buf);
        assert_eq!(l.last_lap_ms, 83_456);
        assert_eq!(l.sector1_ms, 61_500);
        assert_eq!((l.lap_distance_m, l.lap, l.pit_status), (1234.5, 7, 2));
        assert!(l.lap_invalid);
        assert_eq!(l.driver_status, 3);

        let mut p = PacketBuilder::new(Format::F2026, id::SESSION_HISTORY, 1, 0.0, 0);
        p.put(HEADER_LEN, &[0, 2]);
        let lap2 = HEADER_LEN + 7 + 14;
        p.put(lap2, &90_000u32.to_le_bytes())
            .put(lap2 + 4, &30_000u16.to_le_bytes())
            .put(lap2 + 7, &29_000u16.to_le_bytes())
            .put(lap2 + 10, &31_000u16.to_le_bytes())
            .put(lap2 + 13, &[0x0E]);
        let s = SessionHistory::parse(&p.buf);
        assert_eq!(s.laps.len(), 2);
        assert_eq!(s.laps[1].lap_time_ms, 90_000);
        assert_eq!(s.laps[1].sectors_ms, [30_000, 29_000, 31_000]);
        assert!(!s.laps[1].lap_valid());
    }

    #[test]
    fn session_and_events_parse_to_the_end() {
        for format in [Format::F2025, Format::F2026] {
            let mut p = PacketBuilder::new(format, id::SESSION, 1, 0.0, 0);
            p.put(HEADER_LEN, &[3, 30, 22, 5])
                .put(HEADER_LEN + 4, &5_793u16.to_le_bytes())
                .put(HEADER_LEN + 6, &[18, 10, 0])
                .put(745, &1900.0f32.to_le_bytes());
            let h = Header::parse(&p.buf).unwrap();
            let s = Session::parse(&h, &p.buf);
            assert_eq!((s.weather, s.track_temp_c, s.air_temp_c), (3, 30, 22));
            assert_eq!(
                (s.track_length_m, s.session_type, s.track_id),
                (5793, 18, 10)
            );
            assert_eq!(s.sector2_start_m, 1900.0);
        }
        let mut p = PacketBuilder::new(Format::F2026, id::EVENT, 1, 0.0, 0);
        p.put(HEADER_LEN, b"FLBK")
            .put(HEADER_LEN + 4, &600u32.to_le_bytes())
            .put(HEADER_LEN + 8, &10.0f32.to_le_bytes());
        assert_eq!(
            Event::parse(&p.buf),
            Event::Flashback {
                frame: 600,
                session_time: 10.0
            }
        );
    }

    #[test]
    fn participants_and_status_per_format() {
        let mut p = PacketBuilder::new(Format::F2025, id::PARTICIPANTS, 1, 0.0, 2);
        let at = p.car(2, 0);
        p.put(at + 3, &[7]).put(at + 5, &[44]).put(at + 39, &[1]);
        let h = Header::parse(&p.buf).unwrap();
        let part = Participant::parse(&h, &p.buf);
        assert_eq!(
            (part.team_id, part.race_number, part.telemetry_public),
            (7, 44, true)
        );

        let mut p = PacketBuilder::new(Format::F2026, id::PARTICIPANTS, 1, 0.0, 2);
        let at = p.car(2, 0);
        p.put(at + 5, &300u16.to_le_bytes()).put(at + 8, &[44]);
        let h = Header::parse(&p.buf).unwrap();
        assert_eq!(Participant::parse(&h, &p.buf).team_id, 300);

        for (format, deployed_at) in [(Format::F2025, 50), (Format::F2026, 54)] {
            let mut p = PacketBuilder::new(format, id::CAR_STATUS, 1, 0.0, 0);
            let at = p.car(0, 0);
            p.put(at + 3, &[56])
                .put(at + 5, &12.5f32.to_le_bytes())
                .put(at + 37, &4.0e6f32.to_le_bytes())
                .put(at + deployed_at, &1.0e6f32.to_le_bytes());
            let h = Header::parse(&p.buf).unwrap();
            let s = CarStatus::parse(&h, &p.buf);
            assert_eq!((s.front_brake_bias, s.fuel_kg), (56, 12.5));
            assert_eq!((s.ers_store_j, s.ers_deployed_lap_j), (4.0e6, 1.0e6));
        }
    }
}
