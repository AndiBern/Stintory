// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Client side of the Rundenzeit server (docs/PLAN.md §9.3, §9.5):
//!
//! - [`DeviceLogin`]: connect a desktop app through the system browser
//!   (loopback redirect + PKCE);
//! - [`Client`]: the server's device and sync API;
//! - [`sync_once`]: push local changes, pull the other devices' changes.

pub mod client;
pub mod engine;
pub mod login;

pub use client::{Client, SyncError, normalize_server_url};
pub use engine::{SyncReport, sync_once};
pub use login::DeviceLogin;
