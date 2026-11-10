// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Domain model of Rundenzeit: sessions, laps, telemetry channels, and the
//! on-disk lap telemetry format (one Parquet file per lap).
//!
//! This crate is MIT OR Apache-2.0 licensed so that other tools can read and
//! write Rundenzeit data. It must not depend on any copyleft crate.

pub mod api;
pub mod capture;
pub mod channel;
pub mod game;
pub mod ids;
pub mod remote;
#[cfg(feature = "rzx")]
pub mod rzx;
pub mod session;
pub mod telemetry;

pub use channel::{Channel, ChannelData, Unit};
pub use game::{GameId, SessionType};
pub use ids::{BlobHash, LapId, SessionId};
pub use session::{Lap, LapFlags, NewLap, Origin, Session};
pub use telemetry::{LapTelemetry, TelemetryError};
