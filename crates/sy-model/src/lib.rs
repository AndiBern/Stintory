// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Domain model of Stintory: sessions, laps, telemetry channels, and the
//! on-disk lap telemetry format (one Parquet file per lap).
//!
//! This crate is MIT OR Apache-2.0 licensed so that other tools can read and
//! write Stintory data. It must not depend on any copyleft crate.

pub mod api;
#[cfg(feature = "bundle")]
pub mod bundle;
pub mod capture;
pub mod channel;
pub mod game;
pub mod ids;
pub mod remote;
pub mod session;
pub mod telemetry;

pub use channel::{Channel, ChannelData, Unit};
pub use game::{GameId, SessionType};
pub use ids::{BlobHash, LapId, SessionId};
pub use session::{Lap, LapFlags, NewLap, Origin, Session};
pub use telemetry::{LapTelemetry, TelemetryError};
