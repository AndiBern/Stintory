// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The interface between game adapters and the capture pipeline
//! (docs/PLAN.md §5).
//!
//! A game adapter has two parts:
//!
//! - a [`RawSource`] reads the game (shared memory, UDP, file) and returns
//!   [`RawRecord`]s. Data about other cars and drivers must already be
//!   removed (docs/PLAN.md §5.2), because raw records are spooled to disk.
//! - a [`Decoder`] turns raw records into [`SourceEvent`]s for the recorder.
//!
//! The split makes live capture, crash recovery (replaying a spool file) and
//! file import use exactly the same decoding code.

use time::OffsetDateTime;

use crate::session::Conditions;
use crate::{GameId, SessionType, Unit};

/// Storage type of a channel's samples in the lap file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    /// 32-bit float
    F32,
    /// 64-bit float
    F64,
    /// 32-bit integer (gear, enumerations)
    I32,
    /// Flag (non-zero = true)
    Bool,
}

/// Declares one telemetry channel a source delivers in every [`Frame`].
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelSpec {
    /// Canonical or namespaced channel name (see `sy_model::channel::names`)
    pub name: String,
    /// Unit after conversion by the source (SI)
    pub unit: Unit,
    /// How samples are stored
    pub kind: ValueKind,
}

impl ChannelSpec {
    /// Creates a channel spec.
    pub fn new(name: impl Into<String>, unit: Unit, kind: ValueKind) -> Self {
        Self {
            name: name.into(),
            unit,
            kind,
        }
    }
}

/// Information about the session a source is reading.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionMeta {
    /// Identifies the game session. A different key means a new session;
    /// the same key with changed fields is an update of the current one.
    pub session_key: String,
    /// Game
    pub game: GameId,
    /// Game version, if known
    pub game_version: Option<String>,
    /// Canonical track key (incl. layout)
    pub track_key: String,
    /// Human-readable track name incl. layout
    pub track_name: String,
    /// Canonical car key
    pub car_key: String,
    /// Human-readable car name
    pub car_name: String,
    /// Session type
    pub session_type: SessionType,
    /// Wall-clock start of the session (from the game or the capture clock)
    pub started_at: OffsetDateTime,
    /// Weather and temperatures
    pub conditions: Conditions,
    /// Car setup (game-specific JSON)
    pub setup: Option<serde_json::Value>,
    /// Further session info, **filtered to the player only**
    pub session_info: Option<serde_json::Value>,
    /// Nominal sample rate of frames
    pub sample_rate_hz: f32,
    /// Channels delivered in [`Frame::values`], in that order. The recorder
    /// adds `t` (time since lap start) and `dist` itself.
    pub channels: Vec<ChannelSpec>,
}

/// One sample of the player's car.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// Session time in seconds. Increases monotonically except after a
    /// [`SourceEvent::Rewind`].
    pub session_time: f64,
    /// The game's current lap number (the lap being driven).
    pub lap: i32,
    /// Distance from the start/finish line in metres.
    pub lap_dist: f32,
    /// Car is in the pit lane / pit box.
    pub in_pits: bool,
    /// Values of [`SessionMeta::channels`], same order. Converted to the
    /// declared [`ValueKind`] when the lap is stored.
    pub values: Vec<f64>,
}

/// Events emitted by a [`Source`].
#[derive(Debug, Clone, PartialEq)]
pub enum SourceEvent {
    /// A session started, or the info of the current session changed.
    Session(Box<SessionMeta>),
    /// One sample.
    Frame(Frame),
    /// The game reported the official time of a completed lap. May arrive a
    /// little after the first frame of the next lap.
    LapTime {
        /// Number of the completed lap (same numbering as [`Frame::lap`])
        lap: i32,
        /// Official lap time
        lap_time_ms: u32,
        /// Official sector times, if the game reports them
        sector_times_ms: Vec<u32>,
    },
    /// A lap is invalid.
    Invalidate {
        /// Lap number (same numbering as [`Frame::lap`]); `None` means the
        /// lap currently being driven.
        lap: Option<i32>,
        /// Human-readable reason, e.g. "off track"
        reason: String,
    },
    /// The game rewound time (flashback, reset): everything at or after
    /// `to_session_time` did not happen.
    Rewind {
        /// Session time the game jumped back to
        to_session_time: f64,
    },
    /// The lap being driven was interrupted without crossing the line (tow,
    /// reset to pits, player left the car). It is stored as incomplete.
    LapAborted {
        /// Human-readable reason, e.g. "towed"
        reason: String,
    },
    /// The session ended (or the game was closed / the player left the car
    /// for good).
    SessionEnd,
}

/// Errors of a source or decoder.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// The data from the game could not be understood.
    #[error("invalid data from game: {0}")]
    InvalidData(String),
    /// I/O error (files, sockets)
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// One raw record read from a game, in the game adapter's own format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRecord {
    /// Record kind, defined by the game adapter
    pub kind: u8,
    /// Raw bytes, already filtered to the player's car
    pub payload: Vec<u8>,
}

/// Reads raw records from a game.
pub trait RawSource {
    /// The game this source reads.
    fn game(&self) -> GameId;

    /// Waits up to `timeout` for the next record. `Ok(None)` means nothing
    /// arrived within the timeout (live sources) or the input ended (file
    /// sources).
    fn next_record(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<Option<RawRecord>, SourceError>;
}

/// Turns raw records into events for the recorder.
pub trait Decoder {
    /// Decodes one record. May return zero or more events.
    fn decode(&mut self, record: &RawRecord) -> Result<Vec<SourceEvent>, SourceError>;

    /// Called when the input ended; returns final events (e.g. `SessionEnd`).
    fn finish(&mut self) -> Vec<SourceEvent>;
}
