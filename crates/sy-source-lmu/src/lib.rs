// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Le Mans Ultimate adapter for Stintory.
//!
//! Written from scratch from Studio 397's `SharedMemoryInterface.hpp` and
//! `InternalsPlugin.hpp` (shipped with LMU in `Support\SharedMemoryInterface`).
//! No code from other telemetry tools is used (docs/PLAN.md §12.2).
//! Behaviour still to be verified with recordings is listed in
//! docs/games/lmu.md.
//!
//! - [`layout`]: the layout of the shared memory `LMU_Data`
//! - [`records`]: the player-only records that are spooled, and parsing them
//! - `live` (Windows only): reading the shared memory ([`LiveSource`])
//! - [`rest`]: the game's local REST API (GET only): is the game running,
//!   the car setup
//! - [`decoder`]: raw records → `SourceEvent`s ([`LmuDecoder`])
//! - [`sim`]: synthetic records for tests

pub mod decoder;
pub mod layout;
#[cfg(windows)]
pub mod live;
pub mod records;
pub mod rest;
pub mod sim;

pub use decoder::LmuDecoder;
#[cfg(windows)]
pub use live::{LiveSource, Probe};
