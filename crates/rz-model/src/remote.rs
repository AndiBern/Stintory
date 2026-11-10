// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Types exchanged between Rundenzeit clients and a Rundenzeit server
//! (`/api/v1`): the account of the logged-in user, connected devices and
//! sync (docs/PLAN.md §9.5). Serialized in camelCase; sessions and laps use
//! their own serialization (the same as in `.rzx` files).

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ids::SessionId;
use crate::session::{Lap, Session};

/// The logged-in user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    /// Id
    pub id: Uuid,
    /// Login name
    pub username: String,
    /// Name shown to others
    pub display_name: String,
    /// Server admin
    pub is_admin: bool,
    /// TOTP is set up
    pub totp_enabled: bool,
    /// Must set a new password first
    pub must_change_password: bool,
    /// Must set up a second factor first (enforced by the admin)
    pub two_factor_required: bool,
}

/// A connected desktop client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    /// Id
    pub id: Uuid,
    /// Name chosen at login, e.g. the computer's name
    pub name: String,
    /// Connected since
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last request
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
    /// Address of the last request
    pub last_ip: Option<String>,
}

/// A new device token (shown once): `POST /device-login/token`,
/// `POST /me/devices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewDevice {
    /// The token, `rzd_…`
    pub token: String,
    /// The device
    pub device: DeviceInfo,
    /// Whose device it is
    pub user: Me,
}

/// `GET /devices/current`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentDevice {
    /// The device (None when called from a browser)
    pub device: Option<DeviceInfo>,
    /// The user
    pub user: Me,
}

/// `POST /device-login/token`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenRequest {
    /// From the redirect to the app
    pub code: String,
    /// PKCE verifier (43–128 characters)
    pub code_verifier: String,
}

/// `POST /sync/blobs/missing` request: telemetry file hashes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobHashes {
    /// SHA-256 hex hashes
    pub hashes: Vec<String>,
}

/// `POST /sync/blobs/missing` answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingBlobs {
    /// The hashes the server doesn't have
    pub missing: Vec<String>,
}

/// `POST /sync/push`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPush {
    /// New or changed sessions
    #[serde(default)]
    pub sessions: Vec<Session>,
    /// New or changed laps (their sessions must be on the server or in
    /// `sessions`, their telemetry uploaded)
    #[serde(default)]
    pub laps: Vec<Lap>,
    /// Sessions deleted on the device
    #[serde(default)]
    pub deleted_sessions: Vec<SessionId>,
}

/// Answer of `POST /sync/push`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPushResult {
    /// Sessions written
    pub sessions: u32,
    /// Laps written
    pub laps: u32,
    /// Sessions deleted
    pub deleted_sessions: u32,
}

/// One page of `GET /sync/changes?since=…`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncChanges {
    /// Changed sessions, plus the sessions of changed laps
    pub sessions: Vec<Session>,
    /// Changed laps
    pub laps: Vec<Lap>,
    /// Deleted sessions
    pub deleted_sessions: Vec<SessionId>,
    /// Pass as `since` next time
    pub cursor: i64,
    /// There are more changes: ask again right away
    pub more: bool,
}
