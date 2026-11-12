// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stintory's Proton bridge (docs/PLAN.md §3, docs/games/proton.md).
//!
//! Wine doesn't expose a Windows program's named shared memory to native
//! Linux processes. `sy-bridge.exe` runs inside the game's Wine/Proton
//! prefix (started by Proton with the game), reads the game with the same
//! readers as the Windows app and streams the raw records over a localhost
//! TCP connection to the native Stintory app, which spools, decodes and
//! records them exactly like on Windows.
//!
//! ```text
//! game ─shm─► sy-bridge.exe (in the prefix) ─127.0.0.1:20790─► Stintory (Linux)
//! ```
//!
//! - [`protocol`]: the messages and their framing
//! - [`server`]: the bridge's loop ([`server::Bridge`]) over [`server::Game`]s
//! - `windows` (Windows only): the real games, LMU and iRacing
//! - [`sim`]: a simulated LMU, for tests and trying it without the game
//! - [`client`]: the app's side ([`client::BridgeClient`],
//!   [`client::BridgeSource`])

pub mod client;
pub mod protocol;
pub mod server;
pub mod sim;
#[cfg(windows)]
pub mod windows;

pub use client::{BridgeClient, BridgeSource, BridgeView, SharedView};
pub use protocol::{BridgeError, DEFAULT_PORT, GameState};
