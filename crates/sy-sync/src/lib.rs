// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Client side of the Stintory server (docs/ARCHITECTURE.md §9.3, §9.5):
//!
//! - [`DeviceLogin`]: connect a desktop app through the system browser
//!   (loopback redirect + PKCE);
//! - [`Client`]: the server's device and sync API;
//! - [`check_version`]: the app and the server only sync when both run the
//!   same version;
//! - [`sync_once`]: push local changes, pull the other devices' changes,
//!   drop kept laps of others that aren't shared anymore;
//! - [`fetch_shared_session`]: keep a session someone shared as a reference.

pub mod client;
pub mod engine;
pub mod login;
pub mod version;

pub use client::{Client, SyncError, normalize_server_url};
pub use engine::{SyncReport, fetch_shared_session, sync_once};
pub use login::DeviceLogin;
pub use version::{VERSION, check_version};
