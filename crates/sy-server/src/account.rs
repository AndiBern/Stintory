// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The user's own account: password, profile, TOTP, recovery codes and
//! browser sessions (`/me/...`).
//!
//! These endpoints accept restricted sessions ([`AccountAuth`]), because
//! they are how a user resolves the restriction (new password, 2FA setup).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use deadpool_postgres::GenericClient;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::{AccountAuth, ClientIp, Me, me as me_view};
use crate::crypto;
use crate::error::{ApiError, ApiResult};
use crate::state::{ServerSettings, SharedState, audit};

/// Number of recovery codes per user.
pub const RECOVERY_CODES: usize = 10;

/// `GET /me`
pub async fn me(
    State(state): State<SharedState>,
    AccountAuth(session): AccountAuth,
) -> ApiResult<Json<Me>> {
    let settings = ServerSettings::load(&state.conn().await?).await?;
    Ok(Json(me_view(&session.user, &settings)))
}

/// `PATCH /me`
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMe {
    /// New display name
    pub display_name: Option<String>,
}

/// Changes the display name.
pub async fn update_me(
    State(state): State<SharedState>,
    AccountAuth(session): AccountAuth,
    Json(req): Json<UpdateMe>,
) -> ApiResult<StatusCode> {
    if let Some(name) = req.display_name {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err(ApiError::bad_request(
                "the display name must have 1–64 characters",
            ));
        }
        state
            .conn()
            .await?
            .execute(
                "UPDATE users SET display_name = $2, updated_at = now() WHERE id = $1",
                &[&session.user.id, &name],
            )
            .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn check_password(state: &SharedState, user: Uuid, password: &str) -> ApiResult<()> {
    let hash: String = state
        .conn()
        .await?
        .query_one("SELECT password_hash FROM users WHERE id = $1", &[&user])
        .await?
        .get(0);
    if crypto::verify_password(password.to_owned(), Some(hash)).await? {
        Ok(())
    } else {
        Err(ApiError::forbidden("wrong password").with_code("wrong_password"))
    }
}

/// `POST /me/password`
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePassword {
    /// Current password
    pub current_password: String,
    /// New password
    pub new_password: String,
}

/// Sets a new password and ends all other sessions.
pub async fn change_password(
    State(state): State<SharedState>,
    AccountAuth(session): AccountAuth,
    ip: ClientIp,
    Json(req): Json<ChangePassword>,
) -> ApiResult<StatusCode> {
    let user = &session.user;
    if !state.limiter.allowed(
        &format!("user:{}", user.username),
        crate::ratelimit::PER_ACCOUNT,
    ) {
        return Err(ApiError::too_many_requests());
    }
    if let Err(e) = check_password(&state, user.id, &req.current_password).await {
        state.limiter.fail(&format!("user:{}", user.username));
        return Err(e);
    }
    crypto::check_password_policy(&req.new_password)?;
    if req.new_password == req.current_password {
        return Err(ApiError::bad_request(
            "the new password must differ from the current one",
        ));
    }
    let hash = crypto::hash_password(req.new_password).await?;
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    tx.execute(
        "UPDATE users SET password_hash = $2, must_change_password = false, updated_at = now()
         WHERE id = $1",
        &[&user.id, &hash],
    )
    .await?;
    tx.execute(
        "DELETE FROM web_sessions WHERE user_id = $1 AND token_hash != $2",
        &[&user.id, &session.token_hash],
    )
    .await?;
    tx.commit().await?;
    audit(
        &db,
        Some(user.id),
        "password_changed",
        ip.0.as_deref(),
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Answer of `POST /me/totp/setup`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TotpSetup {
    /// Secret in base32, for manual entry
    pub secret: String,
    /// `otpauth://` URI
    pub uri: String,
    /// QR code of the URI (SVG)
    pub qr_svg: String,
}

/// Starts a TOTP setup: a new secret that becomes active once a code is
/// confirmed. Replaces an unconfirmed earlier setup.
pub async fn totp_setup(
    State(state): State<SharedState>,
    AccountAuth(session): AccountAuth,
) -> ApiResult<Json<TotpSetup>> {
    let user = &session.user;
    if user.totp_enabled {
        return Err(ApiError::conflict(
            "two-factor authentication is already set up",
        ));
    }
    let secret = crypto::random_bytes::<20>();
    let sealed = state.secrets.seal(user.id, &secret)?;
    state
        .conn()
        .await?
        .execute(
            "UPDATE users SET totp_secret = $2, totp_enabled_at = NULL, totp_last_step = NULL
             WHERE id = $1",
            &[&user.id, &sealed],
        )
        .await?;
    let host = state
        .config
        .public_origin()
        .split("://")
        .nth(1)
        .unwrap_or("Stintory")
        .to_owned();
    let uri = crypto::totp_uri(&format!("Stintory ({host})"), &user.username, &secret);
    Ok(Json(TotpSetup {
        secret: crypto::base32(&secret),
        qr_svg: crypto::qr_svg(&uri)?,
        uri,
    }))
}

/// A code from the authenticator app.
#[derive(Debug, Deserialize)]
pub struct CodeRequest {
    /// 6-digit code
    pub code: String,
}

/// New recovery codes, shown once.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryCodes {
    /// Codes, each usable once
    pub recovery_codes: Vec<String>,
}

async fn new_recovery_codes(
    state: &SharedState,
    db: &impl GenericClient,
    user: Uuid,
) -> ApiResult<Vec<String>> {
    db.execute("DELETE FROM recovery_codes WHERE user_id = $1", &[&user])
        .await?;
    let codes: Vec<String> = (0..RECOVERY_CODES)
        .map(|_| crypto::new_recovery_code())
        .collect();
    for code in &codes {
        let hash = state
            .secrets
            .mac(crypto::normalize_recovery_code(code).as_bytes());
        db.execute(
            "INSERT INTO recovery_codes (user_id, code_hash) VALUES ($1, $2)",
            &[&user, &hash],
        )
        .await?;
    }
    Ok(codes)
}

/// Confirms the TOTP setup with a code; returns the recovery codes.
pub async fn totp_confirm(
    State(state): State<SharedState>,
    AccountAuth(session): AccountAuth,
    ip: ClientIp,
    Json(req): Json<CodeRequest>,
) -> ApiResult<Json<RecoveryCodes>> {
    let user = &session.user;
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    let row = tx
        .query_one(
            "SELECT totp_secret, totp_enabled_at IS NOT NULL FROM users WHERE id = $1 FOR UPDATE",
            &[&user.id],
        )
        .await?;
    let (sealed, enabled): (Option<Vec<u8>>, bool) = (row.get(0), row.get(1));
    if enabled {
        return Err(ApiError::conflict(
            "two-factor authentication is already set up",
        ));
    }
    let Some(sealed) = sealed else {
        return Err(ApiError::bad_request("start the setup first"));
    };
    let secret = state.secrets.open(user.id, &sealed)?;
    let Some(step) = crypto::totp_verify(&secret, &req.code, crypto::totp_step_now(), None) else {
        return Err(
            ApiError::bad_request("wrong code – check the time on your device")
                .with_code("wrong_code"),
        );
    };
    tx.execute(
        "UPDATE users SET totp_enabled_at = now(), totp_last_step = $2, updated_at = now()
         WHERE id = $1",
        &[&user.id, &(step as i64)],
    )
    .await?;
    let codes = new_recovery_codes(&state, &tx, user.id).await?;
    tx.commit().await?;
    audit(
        &db,
        Some(user.id),
        "totp_enabled",
        ip.0.as_deref(),
        serde_json::json!({}),
    )
    .await;
    Ok(Json(RecoveryCodes {
        recovery_codes: codes,
    }))
}

/// Confirms an action with the account password.
#[derive(Debug, Deserialize)]
pub struct PasswordConfirm {
    /// Current password
    pub password: String,
}

/// `DELETE /me/totp`: turns TOTP off (not while the admin enforces 2FA).
pub async fn totp_disable(
    State(state): State<SharedState>,
    AccountAuth(session): AccountAuth,
    ip: ClientIp,
    Json(req): Json<PasswordConfirm>,
) -> ApiResult<StatusCode> {
    let user = &session.user;
    check_password(&state, user.id, &req.password).await?;
    let db = state.conn().await?;
    if ServerSettings::load(&db).await?.enforce_2fa && !user.has_passkey {
        return Err(ApiError::forbidden(
            "two-factor authentication is required on this server (add a passkey first)",
        ));
    }
    db.execute(
        "UPDATE users SET totp_secret = NULL, totp_enabled_at = NULL, totp_last_step = NULL,
            updated_at = now()
         WHERE id = $1",
        &[&user.id],
    )
    .await?;
    db.execute("DELETE FROM recovery_codes WHERE user_id = $1", &[&user.id])
        .await?;
    audit(
        &db,
        Some(user.id),
        "totp_disabled",
        ip.0.as_deref(),
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /me/recovery-codes`: replaces all recovery codes.
pub async fn regenerate_recovery_codes(
    State(state): State<SharedState>,
    AccountAuth(session): AccountAuth,
    ip: ClientIp,
    Json(req): Json<PasswordConfirm>,
) -> ApiResult<Json<RecoveryCodes>> {
    let user = &session.user;
    check_password(&state, user.id, &req.password).await?;
    if !user.totp_enabled {
        return Err(ApiError::bad_request(
            "set up two-factor authentication first",
        ));
    }
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    let codes = new_recovery_codes(&state, &tx, user.id).await?;
    tx.commit().await?;
    audit(
        &db,
        Some(user.id),
        "recovery_codes_regenerated",
        ip.0.as_deref(),
        serde_json::json!({}),
    )
    .await;
    Ok(Json(RecoveryCodes {
        recovery_codes: codes,
    }))
}

/// One browser session in the account page.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    /// Opaque id
    pub id: String,
    /// The session making this request
    pub current: bool,
    /// Browser
    pub user_agent: Option<String>,
    /// Address
    pub ip: Option<String>,
    /// Created
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    /// Last used
    #[serde(with = "time::serde::rfc3339")]
    pub last_seen_at: time::OffsetDateTime,
}

/// `GET /me/sessions`
pub async fn sessions(
    State(state): State<SharedState>,
    AccountAuth(session): AccountAuth,
) -> ApiResult<Json<Vec<SessionInfo>>> {
    let rows = state
        .conn()
        .await?
        .query(
            "SELECT token_hash, user_agent, ip, created_at, last_seen_at FROM web_sessions
             WHERE user_id = $1 AND expires_at > now() ORDER BY last_seen_at DESC",
            &[&session.user.id],
        )
        .await?;
    Ok(Json(
        rows.iter()
            .map(|r| {
                let hash: Vec<u8> = r.get(0);
                SessionInfo {
                    id: hex::encode(&hash[..16]),
                    current: hash == session.token_hash,
                    user_agent: r.get(1),
                    ip: r.get(2),
                    created_at: r.get(3),
                    last_seen_at: r.get(4),
                }
            })
            .collect(),
    ))
}

/// `DELETE /me/sessions/{id}`: ends one of the user's sessions.
pub async fn end_session(
    State(state): State<SharedState>,
    AccountAuth(session): AccountAuth,
    ip: ClientIp,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let prefix = hex::decode(&id).map_err(|_| ApiError::not_found("session"))?;
    if prefix.len() != 16 {
        return Err(ApiError::not_found("session"));
    }
    let db = state.conn().await?;
    let n = db
        .execute(
            "DELETE FROM web_sessions WHERE user_id = $1 AND substring(token_hash from 1 for 16) = $2",
            &[&session.user.id, &prefix],
        )
        .await?;
    if n == 0 {
        return Err(ApiError::not_found("session"));
    }
    audit(
        &db,
        Some(session.user.id),
        "session_ended",
        ip.0.as_deref(),
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}
