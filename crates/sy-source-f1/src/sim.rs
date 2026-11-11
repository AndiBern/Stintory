// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Synthetic packets, for tests and for trying the app without the game
//! (`cargo run -p sy-source-f1 --example simulate`).
//!
//! [`SimGame`] drives a car round an oval at constant speed and produces the
//! packets the game would send for it. The packets follow the layouts in
//! [`crate::packets`]; they are not recordings of the real game.

use crate::packets::{Format, HEADER_LEN, Header, car_array, id, packet_size};

/// Builds one packet: the spec's size, a header, zeros otherwise.
#[derive(Debug, Clone)]
pub struct PacketBuilder {
    /// The packet
    pub buf: Vec<u8>,
}

impl PacketBuilder {
    /// A packet with a header.
    ///
    /// # Panics
    ///
    /// If the format has no such packet.
    pub fn new(format: Format, packet_id: u8, uid: u64, time: f32, player: u8) -> Self {
        let size = packet_size(format, packet_id).expect("known packet");
        let mut buf = vec![0u8; size];
        buf[0..2].copy_from_slice(&format.packet_format().to_le_bytes());
        buf[2] = 25;
        buf[3] = 1;
        buf[4] = 7;
        buf[5] = 1;
        buf[6] = packet_id;
        buf[7..15].copy_from_slice(&uid.to_le_bytes());
        buf[15..19].copy_from_slice(&time.to_le_bytes());
        let frame = (time * 60.0) as u32;
        buf[19..23].copy_from_slice(&frame.to_le_bytes());
        buf[23..27].copy_from_slice(&frame.to_le_bytes());
        buf[27] = player;
        buf[28] = 255;
        Self { buf }
    }

    /// Writes bytes at an offset.
    pub fn put(&mut self, at: usize, bytes: &[u8]) -> &mut Self {
        self.buf[at..at + bytes.len()].copy_from_slice(bytes);
        self
    }

    /// Offset of byte `field` of car `car`'s entry in the per-car array.
    ///
    /// # Panics
    ///
    /// If the packet has no per-car array.
    pub fn car(&self, car: u8, field: usize) -> usize {
        let h = Header::parse(&self.buf).expect("valid header");
        let (offset, stride) = car_array(h.format, h.packet_id).expect("car array");
        offset + stride * usize::from(car) + field
    }
}

/// A scripted game session: set the fields, then take the packets of the
/// current frame.
#[derive(Debug, Clone)]
pub struct SimGame {
    /// UDP format
    pub format: Format,
    /// Session id
    pub uid: u64,
    /// Player's car index
    pub player: u8,
    /// Session time, s
    pub time: f32,
    /// Lap being driven
    pub lap: u8,
    /// Distance in the lap, m
    pub dist: f32,
    /// Track length, m
    pub track_length: u16,
    /// Time of the last lap, ms
    pub last_lap_ms: u32,
    /// Sector 1 and 2 times of the current lap, ms
    pub sectors: (u16, u16),
    /// The current lap is invalid
    pub invalid: bool,
    /// Pit status (0 none, 1 pitting, 2 in the pit area)
    pub pit: u8,
    /// Driver status (0 garage, 1 flying lap, 2 in-lap, 3 out-lap, 4 on track)
    pub driver_status: u8,
    /// Session type (18 time trial, 15 race, …)
    pub session_type: u8,
    /// Track id
    pub track_id: i8,
    /// Team id
    pub team: u8,
    /// Speed, km/h
    pub speed_kmh: u16,
    /// Completed laps for the session history: (time ms, valid)
    pub history: Vec<(u32, bool)>,
}

impl SimGame {
    /// A time trial at Spa (track 10) with a 1000 m track.
    pub fn new(format: Format) -> Self {
        Self {
            format,
            uid: 0xF1,
            player: 3,
            time: 1.0,
            lap: 1,
            dist: 0.0,
            track_length: 1000,
            last_lap_ms: 0,
            sectors: (0, 0),
            invalid: false,
            pit: 0,
            driver_status: 1,
            session_type: 18,
            track_id: 10,
            team: 4,
            speed_kmh: 180,
            history: Vec::new(),
        }
    }

    fn packet(&self, id: u8) -> PacketBuilder {
        PacketBuilder::new(self.format, id, self.uid, self.time, self.player)
    }

    /// Session packet.
    pub fn session(&self) -> Vec<u8> {
        let mut p = self.packet(id::SESSION);
        p.put(HEADER_LEN, &[0, 32, 24, 5])
            .put(HEADER_LEN + 4, &self.track_length.to_le_bytes())
            .put(HEADER_LEN + 6, &[self.session_type, self.track_id as u8, 0]);
        p.buf
    }

    /// Participants packet.
    pub fn participants(&self) -> Vec<u8> {
        let mut p = self.packet(id::PARTICIPANTS);
        let at = p.car(
            self.player,
            if self.format == Format::F2025 { 3 } else { 5 },
        );
        p.put(at, &[self.team]);
        p.buf
    }

    /// Event packet.
    pub fn event(&self, code: &[u8; 4], details: &[u8]) -> Vec<u8> {
        let mut p = self.packet(id::EVENT);
        p.put(HEADER_LEN, code).put(HEADER_LEN + 4, details);
        p.buf
    }

    /// Flashback event to a session time.
    pub fn flashback(&self, to: f32) -> Vec<u8> {
        let mut details = ((to * 60.0) as u32).to_le_bytes().to_vec();
        details.extend_from_slice(&to.to_le_bytes());
        self.event(b"FLBK", &details)
    }

    /// A penalty for the player's car in the current lap (types and
    /// infringements: see [`crate::tables::invalidation_reason`]).
    pub fn penalty(&self, penalty_type: u8, infringement: u8) -> Vec<u8> {
        self.event(
            b"PENA",
            &[penalty_type, infringement, self.player, 255, 0, self.lap, 0],
        )
    }

    /// Session history of the player's car ([`history`](Self::history)
    /// plus the lap being driven).
    pub fn session_history(&self) -> Vec<u8> {
        let mut p = self.packet(id::SESSION_HISTORY);
        p.put(HEADER_LEN, &[self.player, (self.history.len() + 1) as u8]);
        for (i, (ms, valid)) in self.history.iter().enumerate() {
            let at = HEADER_LEN + 7 + 14 * i;
            p.put(at, &ms.to_le_bytes())
                .put(at + 13, &[if *valid { 0x0F } else { 0x00 }]);
        }
        p.buf
    }

    /// Position on an oval with the track's length, metres (x, z).
    fn position(&self) -> (f32, f32) {
        let r = f32::from(self.track_length) / std::f32::consts::TAU;
        let a = self.dist / r;
        (r * a.cos(), r * a.sin())
    }

    /// The packets of one frame: lap data, motion, status, extra motion,
    /// (2026) telemetry 2, and the car telemetry that completes the frame.
    pub fn frame(&self) -> Vec<Vec<u8>> {
        let mut lap = self.packet(id::LAP_DATA);
        let at = lap.car(self.player, 0);
        lap.put(at, &self.last_lap_ms.to_le_bytes())
            .put(at + 8, &self.sectors.0.to_le_bytes())
            .put(at + 11, &self.sectors.1.to_le_bytes())
            .put(at + 20, &self.dist.to_le_bytes())
            .put(at + 33, &[self.lap, self.pit])
            .put(at + 37, &[u8::from(self.invalid)])
            .put(at + 44, &[self.driver_status]);
        let mut motion = self.packet(id::MOTION);
        let at = motion.car(self.player, 0);
        let (x, z) = self.position();
        motion
            .put(at, &x.to_le_bytes())
            .put(at + 8, &z.to_le_bytes());
        let mut status = self.packet(id::CAR_STATUS);
        let at = status.car(self.player, 0);
        status
            .put(at + 3, &[56])
            .put(at + 5, &10.0f32.to_le_bytes());
        let motion_ex = self.packet(id::MOTION_EX);
        let mut tel = self.packet(id::CAR_TELEMETRY);
        let at = tel.car(self.player, 0);
        tel.put(at, &self.speed_kmh.to_le_bytes())
            .put(at + 2, &1.0f32.to_le_bytes())
            .put(at + 15, &[6])
            .put(at + 16, &10_500u16.to_le_bytes());
        let mut out = vec![lap.buf, motion.buf, status.buf, motion_ex.buf];
        if self.format == Format::F2026 {
            out.push(self.packet(id::CAR_TELEMETRY_2).buf);
        }
        out.push(tel.buf);
        out
    }

    /// Advances by `dt` seconds at constant speed. Crossing the line starts
    /// the next lap with `last_lap_ms` set (sectors split in thirds) and
    /// adds the finished lap to the history.
    pub fn advance(&mut self, dt: f32) {
        self.time += dt;
        self.dist += f32::from(self.speed_kmh) / 3.6 * dt;
        let len = f32::from(self.track_length);
        if self.dist >= len {
            self.dist -= len;
            let lap_ms = (len / (f32::from(self.speed_kmh) / 3.6) * 1000.0).round() as u32;
            self.last_lap_ms = lap_ms;
            self.history.push((lap_ms, !self.invalid));
            self.lap += 1;
            self.invalid = false;
            self.sectors = (0, 0);
        } else {
            let lap_ms = len / (f32::from(self.speed_kmh) / 3.6) * 1000.0;
            let third = (lap_ms / 3.0) as u16;
            if self.dist >= 2.0 * len / 3.0 {
                self.sectors = (third, third);
            } else if self.dist >= len / 3.0 {
                self.sectors = (third, 0);
            }
        }
    }
}
