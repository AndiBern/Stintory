// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server administration: users, settings, audit log (`/admin/...`).

use axum::Json;
use axum::extract::{Path, Query, State};
use deadpool_postgres::GenericClient;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::{AdminAuth, ClientIp, USER_COLUMNS, User, create_user, normalize_username};
use crate::crypto;
use crate::error::{ApiError, ApiResult};
use crate::state::{ServerSettings, SharedState, audit};

/// A user in the admin list.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminUser {
    /// Id
    pub id: Uuid,
    /// Login name
    pub username: String,
    /// Display name
    pub display_name: String,
    /// Server admin
    pub is_admin: bool,
    /// Disabled
    pub disabled: bool,
    /// Has TOTP
    pub totp_enabled: bool,
    /// Has a passkey
    pub has_passkey: bool,
    /// Must change the password
    pub must_change_password: bool,
    /// Created
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
}

/// `GET /admin/users`
pub async fn list_users(
    State(state): State<SharedState>,
    _: AdminAuth,
) -> ApiResult<Json<Vec<AdminUser>>> {
    let rows = state
        .conn()
        .await?
        .query(
            &format!("SELECT {USER_COLUMNS}, u.created_at FROM users u ORDER BY u.username"),
            &[],
        )
        .await?;
    Ok(Json(
        rows.iter()
            .map(|r| {
                let u = User::from_row(r, 0);
                AdminUser {
                    id: u.id,
                    username: u.username,
                    display_name: u.display_name,
                    is_admin: u.is_admin,
                    disabled: u.disabled,
                    totp_enabled: u.totp_enabled,
                    has_passkey: u.has_passkey,
                    must_change_password: u.must_change_password,
                    created_at: r.get(8),
                }
            })
            .collect(),
    ))
}

/// `POST /admin/users`
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateUser {
    /// Login name
    pub username: String,
    /// Display name (defaults to the username)
    #[serde(default)]
    pub display_name: String,
    /// Make the user an admin
    #[serde(default)]
    pub is_admin: bool,
}

/// A temporary password to hand to the user (shown once).
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemporaryPassword {
    /// User id
    pub id: Option<Uuid>,
    /// The user must change it at the first login
    pub temporary_password: Option<String>,
}

/// Creates an account with a temporary password.
pub async fn create(
    State(state): State<SharedState>,
    AdminAuth(admin): AdminAuth,
    ip: ClientIp,
    Json(req): Json<CreateUser>,
) -> ApiResult<Json<TemporaryPassword>> {
    let username = normalize_username(&req.username)?;
    let password = crypto::temporary_password();
    let hash = crypto::hash_password(password.clone()).await?;
    let db = state.conn().await?;
    let id = create_user(&db, &username, &req.display_name, &hash, req.is_admin, true).await?;
    audit(
        &db,
        Some(admin.user.id),
        "admin_user_created",
        ip.0.as_deref(),
        serde_json::json!({ "user": id, "username": username, "admin": req.is_admin }),
    )
    .await;
    Ok(Json(TemporaryPassword {
        id: Some(id),
        temporary_password: Some(password),
    }))
}

/// `PATCH /admin/users/{id}`
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUser {
    /// Disable or enable the account
    pub disabled: Option<bool>,
    /// Grant or remove admin rights
    pub is_admin: Option<bool>,
    /// Set a new temporary password (ends the user's sessions)
    #[serde(default)]
    pub reset_password: bool,
    /// Remove TOTP, recovery codes and passkeys
    #[serde(default)]
    pub reset_two_factor: bool,
}

/// Changes an account.
pub async fn update(
    State(state): State<SharedState>,
    AdminAuth(admin): AdminAuth,
    ip: ClientIp,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateUser>,
) -> ApiResult<Json<TemporaryPassword>> {
    if id == admin.user.id && (req.disabled == Some(true) || req.is_admin == Some(false)) {
        return Err(ApiError::bad_request(
            "you can't disable yourself or remove your own admin rights",
        ));
    }
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    let user = User::by_id(&tx, id)
        .await?
        .ok_or_else(|| ApiError::not_found("user"))?;
    if let Some(disabled) = req.disabled {
        tx.execute(
            "UPDATE users SET disabled = $2, updated_at = now() WHERE id = $1",
            &[&id, &disabled],
        )
        .await?;
        if disabled {
            tx.execute("DELETE FROM web_sessions WHERE user_id = $1", &[&id])
                .await?;
        }
    }
    if let Some(is_admin) = req.is_admin {
        tx.execute(
            "UPDATE users SET is_admin = $2, updated_at = now() WHERE id = $1",
            &[&id, &is_admin],
        )
        .await?;
    }
    let mut answer = TemporaryPassword::default();
    if req.reset_password || req.reset_two_factor {
        answer.temporary_password =
            reset_user(&tx, id, req.reset_password, req.reset_two_factor).await?;
    }
    let admins: i64 = tx
        .query_one(
            "SELECT count(*) FROM users WHERE is_admin AND NOT disabled",
            &[],
        )
        .await?
        .get(0);
    if admins == 0 {
        return Err(ApiError::bad_request(
            "the server needs at least one active admin",
        ));
    }
    tx.commit().await?;
    audit(
        &db,
        Some(admin.user.id),
        "admin_user_changed",
        ip.0.as_deref(),
        serde_json::json!({
            "user": id, "username": user.username, "disabled": req.disabled,
            "admin": req.is_admin, "resetPassword": req.reset_password,
            "resetTwoFactor": req.reset_two_factor,
        }),
    )
    .await;
    answer.id = Some(id);
    Ok(Json(answer))
}

/// Resets an account's password (returns the new temporary one) and/or its
/// second factor. Ends the user's sessions and disconnects all their desktop
/// apps and AI assistants (a reset is how an account is recovered: nothing
/// someone else may have set up survives it). Also used by
/// `sy-server admin reset-user`.
pub async fn reset_user(
    db: &impl GenericClient,
    id: Uuid,
    password: bool,
    two_factor: bool,
) -> ApiResult<Option<String>> {
    let mut temporary = None;
    if password {
        let pw = crypto::temporary_password();
        let hash = crypto::hash_password(pw.clone()).await?;
        db.execute(
            "UPDATE users SET password_hash = $2, must_change_password = true, updated_at = now()
             WHERE id = $1",
            &[&id, &hash],
        )
        .await?;
        temporary = Some(pw);
    }
    if two_factor {
        db.execute(
            "UPDATE users SET totp_secret = NULL, totp_enabled_at = NULL, totp_last_step = NULL,
                updated_at = now()
             WHERE id = $1",
            &[&id],
        )
        .await?;
        db.execute("DELETE FROM recovery_codes WHERE user_id = $1", &[&id])
            .await?;
        db.execute("DELETE FROM passkeys WHERE user_id = $1", &[&id])
            .await?;
    }
    db.execute("DELETE FROM web_sessions WHERE user_id = $1", &[&id])
        .await?;
    db.execute("DELETE FROM login_challenges WHERE user_id = $1", &[&id])
        .await?;
    crate::account::disconnect_devices(db, id).await?;
    Ok(temporary)
}

/// `GET /admin/settings`
pub async fn get_settings(
    State(state): State<SharedState>,
    _: AdminAuth,
) -> ApiResult<Json<ServerSettings>> {
    Ok(Json(ServerSettings::load(&state.conn().await?).await?))
}

/// `PUT /admin/settings`
pub async fn put_settings(
    State(state): State<SharedState>,
    AdminAuth(admin): AdminAuth,
    ip: ClientIp,
    Json(settings): Json<ServerSettings>,
) -> ApiResult<Json<ServerSettings>> {
    let db = state.conn().await?;
    if settings.enforce_2fa && !admin.user.has_second_factor() {
        return Err(ApiError::bad_request(
            "set up two-factor authentication for yourself before requiring it",
        ));
    }
    settings.save(&db).await?;
    audit(
        &db,
        Some(admin.user.id),
        "admin_settings_changed",
        ip.0.as_deref(),
        serde_json::to_value(&settings)?,
    )
    .await;
    Ok(Json(settings))
}

/// One audit log entry.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    /// When
    #[serde(with = "time::serde::rfc3339")]
    pub at: time::OffsetDateTime,
    /// Username, if the event belongs to a user
    pub username: Option<String>,
    /// Event name, e.g. `login_failed`
    pub event: String,
    /// Client address
    pub ip: Option<String>,
    /// Event details
    pub details: serde_json::Value,
}

/// `GET /admin/audit?limit=`
#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    /// Entries (default 200, at most 1000)
    pub limit: Option<i64>,
}

/// The newest audit log entries.
pub async fn audit_log(
    State(state): State<SharedState>,
    _: AdminAuth,
    Query(q): Query<AuditQuery>,
) -> ApiResult<Json<Vec<AuditEntry>>> {
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    let rows = state
        .conn()
        .await?
        .query(
            "SELECT a.at, u.username, a.event, a.ip, a.details
             FROM audit_log a LEFT JOIN users u ON u.id = a.user_id
             ORDER BY a.id DESC LIMIT $1",
            &[&limit],
        )
        .await?;
    Ok(Json(
        rows.iter()
            .map(|r| AuditEntry {
                at: r.get(0),
                username: r.get(1),
                event: r.get(2),
                ip: r.get(3),
                details: r.get(4),
            })
            .collect(),
    ))
}
