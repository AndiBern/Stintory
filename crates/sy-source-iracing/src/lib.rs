// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! iRacing adapter for Stintory.
//!
//! Written from scratch from the official iRacing SDK 1.20
//! (`irsdk_defines.h`, reference readers `irsdk_utils.cpp` and
//! `irsdk_diskclient.cpp`) and verified against recorded `.ibt` files.
//! No code from other telemetry tools is used (docs/ARCHITECTURE.md §12.2).
//!
//! - [`format`]: the binary format shared by live data and `.ibt` files
//! - [`ibt`]: reading `.ibt` files ([`IbtSource`])
//! - `live` (Windows only): reading the live memory-mapped file
//! - [`decoder`]: raw records → `SourceEvent`s ([`IracingDecoder`])
//! - [`session_info`]: player-only session info from iRacing's YAML

pub mod decoder;
pub mod format;
pub mod ibt;
#[cfg(windows)]
pub mod live;
pub mod session_info;
pub mod yaml;

pub use decoder::IracingDecoder;
pub use ibt::{IbtFile, IbtSource};
#[cfg(windows)]
pub use live::LiveSource;
