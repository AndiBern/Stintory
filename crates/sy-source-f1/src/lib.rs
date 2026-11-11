// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! EA SPORTS F1 25 adapter for Stintory (UDP telemetry, packet formats
//! 2025 and 2026).
//!
//! Written from scratch from EA's UDP specifications for F1 25 (format
//! 2025) and the F1 25 2026 Season Pack (format 2026). No code from other
//! telemetry tools is used (docs/PLAN.md §12.2). Behaviour still to be
//! verified with recordings is listed in docs/games/f1.md.
//!
//! - [`packets`]: packet layouts of both formats
//! - [`filter`]: reduces packets to the player's car before spooling
//! - [`udp`]: receiving (and forwarding) packets ([`UdpSource`])
//! - [`decoder`]: raw records → `SourceEvent`s ([`F1Decoder`])
//! - [`tables`]: names for track, session and other ids
//! - [`sim`]: synthetic packets (tests, trying the app without the game)

pub mod decoder;
pub mod filter;
pub mod packets;
pub mod sim;
pub mod tables;
pub mod udp;

pub use decoder::F1Decoder;
pub use udp::{DEFAULT_PORT, UdpConfig, UdpSource};
