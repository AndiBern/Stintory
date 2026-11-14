// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Desktop clients: device tokens and the browser-based device login
//! (docs/PLAN.md §9.3).
//!
//! # Device login (RFC 8252: loopback redirect + PKCE)
//!
//! 1. The desktop app listens on `http://127.0.0.1:<port>/…` and opens
//!    `<server>/device-login?redirect_uri=…&code_challenge=…&state=…&device_name=…`
//!    in the system browser.
//! 2. The user logs in there (any login method) and approves the device. The
//!    web UI calls `POST /device-login/approve`; the server stores a
//!    single-use code bound to the PKCE challenge and answers with the
//!    redirect URL to the app.
//! 3. The app receives the code and calls `POST /device-login/token` with
//!    the PKCE verifier. Only the app that started the login knows it.
//!
//! Device tokens (`syd_…`) are long-lived, stored hashed, listed on the
//! account page and can be revoked one by one. They give access to data and
//! sync, not to account settings or administration.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use sy_model::remote::{CurrentDevice, DeviceInfo, NewDevice, TokenRequest, TokenScope};

use crate::auth::{AccountAuth, Auth, ClientIp, Session, USER_COLUMNS, User, me};
use crate::crypto;
use crate::error::{ApiError, ApiResult};
use crate::ratelimit::PER_ADDRESS;
use crate::state::{ServerSettings, SharedState, audit};

/// Prefix of device tokens (recognizable in logs and secret scanners).
pub const TOKEN_PREFIX: &str = "syd_";
/// An approved device login must be completed within this.
const CODE_SECONDS: i32 = 120;

/// Looks up a device token (`Authorization: Bearer`).
pub async fn authenticate(state: &SharedState, token: &str, ip: &ClientIp) -> ApiResult<Session> {
    let hash = crypto::token_hash(token);
    let db = state.conn().await?;
    let row = db
        .query_opt(
            &format!(
                "SELECT d.id, d.last_used_at, d.scope, {USER_COLUMNS}
                 FROM device_tokens d JOIN users u ON u.id = d.user_id
                 WHERE d.token_hash = $1 AND d.oauth_client IS NULL AND NOT u.disabled"
            ),
            &[&hash],
        )
        .await?;
    let Some(row) = row else {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "this device is not connected anymore",
        )
        .with_code("device_revoked"));
    };
    let device_id: Uuid = row.get(0);
    let last_used: Option<time::OffsetDateTime> = row.get(1);
    let read_only = row.get::<_, &str>(2) == TokenScope::Read.as_str();
    if last_used.is_none_or(|t| time::OffsetDateTime::now_utc() - t > time::Duration::minutes(5)) {
        db.execute(
            "UPDATE device_tokens SET last_used_at = now(), last_ip = $2 WHERE id = $1",
            &[&device_id, &ip.0],
        )
        .await?;
    }
    Ok(Session {
        user: User::from_row(&row, 3),
        token_hash: Vec::new(),
        csrf_token: String::new(),
        device_id: Some(device_id),
        read_only,
    })
}

fn device_name(name: &str) -> ApiResult<String> {
    let name: String = name.trim().chars().filter(|c| !c.is_control()).collect();
    match name.chars().count() {
        0 => Ok("Desktop".into()),
        1..=64 => Ok(name),
        _ => Err(ApiError::bad_request("the device name is too long")),
    }
}

async fn create_token(
    state: &SharedState,
    user: Uuid,
    name: &str,
    ip: &ClientIp,
    how: &str,
    scope: TokenScope,
) -> ApiResult<NewDevice> {
    let id = Uuid::now_v7();
    let token = format!("{TOKEN_PREFIX}{}", crypto::new_token());
    let db = state.conn().await?;
    db.execute(
        "INSERT INTO device_tokens (id, user_id, token_hash, name, scope)
         VALUES ($1, $2, $3, $4, $5)",
        &[
            &id,
            &user,
            &crypto::token_hash(&token),
            &name,
            &scope.as_str(),
        ],
    )
    .await?;
    audit(
        &db,
        Some(user),
        "device_connected",
        ip.as_deref(),
        serde_json::json!({ "device": id, "name": name, "via": how, "scope": scope.as_str() }),
    )
    .await;
    let user = User::by_id(&db, user)
        .await?
        .ok_or_else(|| ApiError::internal("user vanished"))?;
    let settings = ServerSettings::load(&db).await?;
    Ok(NewDevice {
        token,
        device: DeviceInfo {
            id,
            scope,
            name: name.to_owned(),
            created_at: time::OffsetDateTime::now_utc(),
            last_used_at: None,
            last_ip: None,
        },
        user: me(&user, &settings),
    })
}

// ----- device login -------------------------------------------------------

/// Checks that `uri` is a loopback redirect (RFC 8252 §7.3):
/// `http://127.0.0.1:<port>/<path>` (or `[::1]`, `localhost`).
pub fn check_loopback_redirect(uri: &str) -> ApiResult<()> {
    let bad = || ApiError::bad_request("the redirect address must be http://127.0.0.1:<port>/…");
    let rest = uri.strip_prefix("http://").ok_or_else(bad)?;
    let (authority, path) = rest.split_at(rest.find('/').ok_or_else(bad)?);
    let (host, port) = authority.rsplit_once(':').ok_or_else(bad)?;
    if !matches!(host, "127.0.0.1" | "[::1]" | "localhost") {
        return Err(bad());
    }
    let port: u16 = port.parse().map_err(|_| bad())?;
    if port < 1024 || path.contains(['?', '#', '\\']) || path.contains("..") {
        return Err(bad());
    }
    Ok(())
}

/// `POST /device-login/approve`
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApproveRequest {
    /// Where the app listens
    pub redirect_uri: String,
    /// `BASE64URL(SHA-256(verifier))`
    pub code_challenge: String,
    /// The app's `state`, passed back unchanged
    pub state: String,
    /// Name for the device list
    #[serde(default)]
    pub device_name: String,
}

/// Where the browser goes next.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApproveResponse {
    /// The app's redirect address with `code` and `state`
    pub redirect_to: String,
}

pub(crate) fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The logged-in user approves a device login in the browser.
pub async fn approve(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Json(req): Json<ApproveRequest>,
) -> ApiResult<Json<ApproveResponse>> {
    if session.device_id.is_some() {
        return Err(ApiError::forbidden("approve device logins in the browser"));
    }
    check_loopback_redirect(&req.redirect_uri)?;
    let challenge_ok = req.code_challenge.len() == 43
        && req
            .code_challenge
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !challenge_ok {
        return Err(ApiError::bad_request(
            "invalid code challenge (S256 expected)",
        ));
    }
    if req.state.len() > 256 {
        return Err(ApiError::bad_request("state too long"));
    }
    let name = device_name(&req.device_name)?;
    let code = crypto::new_token();
    let db = state.conn().await?;
    db.execute("DELETE FROM device_codes WHERE expires_at < now()", &[])
        .await?;
    db.execute(
        "INSERT INTO device_codes (code_hash, user_id, code_challenge, device_name, expires_at)
         VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5))",
        &[
            &crypto::token_hash(&code),
            &session.user.id,
            &req.code_challenge,
            &name,
            &f64::from(CODE_SECONDS),
        ],
    )
    .await?;
    Ok(Json(ApproveResponse {
        redirect_to: format!(
            "{}?code={}&state={}",
            req.redirect_uri,
            url_encode(&code),
            url_encode(&req.state)
        ),
    }))
}

/// The app exchanges the code for a device token.
pub async fn token(
    State(state): State<SharedState>,
    ip: ClientIp,
    Json(req): Json<TokenRequest>,
) -> ApiResult<Json<NewDevice>> {
    if !state.limiter.allowed(&ip.limiter_key(), PER_ADDRESS) {
        return Err(ApiError::too_many_requests());
    }
    let invalid = || {
        state.limiter.fail(&ip.limiter_key());
        ApiError::bad_request("the device login is invalid or expired, please try again")
            .with_code("invalid_grant")
    };
    let row = state
        .conn()
        .await?
        .query_opt(
            "DELETE FROM device_codes WHERE code_hash = $1 AND expires_at > now()
             RETURNING user_id, code_challenge, device_name",
            &[&crypto::token_hash(&req.code)],
        )
        .await?;
    let Some(row) = row else {
        return Err(invalid());
    };
    let (user, challenge, name): (Uuid, String, String) = (row.get(0), row.get(1), row.get(2));
    let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(req.code_verifier.as_bytes()));
    if !(43..=128).contains(&req.code_verifier.len()) || expected != challenge {
        return Err(invalid());
    }
    Ok(Json(
        create_token(&state, user, &name, &ip, "device login", TokenScope::Sync).await?,
    ))
}

// ----- managing devices ----------------------------------------------------

async fn device(state: &SharedState, user: Uuid, id: Uuid) -> ApiResult<Option<DeviceInfo>> {
    Ok(list(state, user).await?.into_iter().find(|d| d.id == id))
}

async fn list(state: &SharedState, user: Uuid) -> ApiResult<Vec<DeviceInfo>> {
    let rows = state
        .conn()
        .await?
        .query(
            "SELECT id, name, created_at, last_used_at, last_ip, scope FROM device_tokens
             WHERE user_id = $1 ORDER BY created_at",
            &[&user],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| DeviceInfo {
            id: r.get(0),
            scope: if r.get::<_, &str>(5) == TokenScope::Read.as_str() {
                TokenScope::Read
            } else {
                TokenScope::Sync
            },
            name: r.get(1),
            created_at: r.get(2),
            last_used_at: r.get(3),
            last_ip: r.get(4),
        })
        .collect())
}

/// `GET /devices/current`: who am I (for the desktop app).
pub async fn current(
    State(state): State<SharedState>,
    Auth(session): Auth,
) -> ApiResult<Json<CurrentDevice>> {
    let settings = ServerSettings::load(&state.conn().await?).await?;
    let device = match session.device_id {
        Some(id) => device(&state, session.user.id, id).await?,
        None => None,
    };
    Ok(Json(CurrentDevice {
        device,
        user: me(&session.user, &settings),
    }))
}

/// `DELETE /devices/current`: the app disconnects itself.
pub async fn disconnect(
    State(state): State<SharedState>,
    Auth(session): Auth,
    ip: ClientIp,
) -> ApiResult<StatusCode> {
    let Some(id) = session.device_id else {
        return Err(ApiError::bad_request("not a device"));
    };
    revoke_device(&state, session.user.id, id, &ip).await
}

async fn revoke_device(
    state: &SharedState,
    user: Uuid,
    id: Uuid,
    ip: &ClientIp,
) -> ApiResult<StatusCode> {
    let db = state.conn().await?;
    let n = db
        .execute(
            "DELETE FROM device_tokens WHERE id = $1 AND user_id = $2",
            &[&id, &user],
        )
        .await?;
    if n == 0 {
        return Err(ApiError::not_found("device"));
    }
    audit(
        &db,
        Some(user),
        "device_revoked",
        ip.as_deref(),
        serde_json::json!({ "device": id }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /me/devices`
pub async fn my_devices(
    State(state): State<SharedState>,
    AccountAuth(session): AccountAuth,
) -> ApiResult<Json<Vec<DeviceInfo>>> {
    Ok(Json(list(&state, session.user.id).await?))
}

/// `POST /me/devices`
#[derive(Debug, Deserialize)]
pub struct CreateDevice {
    /// Name for the list
    #[serde(default)]
    pub name: String,
    /// What the token may do (default: sync, for the desktop app)
    #[serde(default)]
    pub scope: TokenScope,
}

/// Creates a device token by hand (for setups where the browser login
/// can't reach the app), or a read-only token for an AI assistant (MCP).
/// Shown once.
pub async fn create(
    State(state): State<SharedState>,
    Auth(session): Auth,
    ip: ClientIp,
    Json(req): Json<CreateDevice>,
) -> ApiResult<Json<NewDevice>> {
    if session.device_id.is_some() {
        return Err(ApiError::forbidden("create device tokens in the browser"));
    }
    let name = device_name(&req.name)?;
    Ok(Json(
        create_token(&state, session.user.id, &name, &ip, "manual", req.scope).await?,
    ))
}

/// `DELETE /me/devices/{id}`: revokes a device; it can't sync anymore.
pub async fn revoke(
    State(state): State<SharedState>,
    AccountAuth(session): AccountAuth,
    ip: ClientIp,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    revoke_device(&state, session.user.id, id, &ip).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_redirects_only() {
        for ok in [
            "http://127.0.0.1:49152/callback",
            "http://[::1]:50000/cb",
            "http://localhost:8123/",
        ] {
            assert!(check_loopback_redirect(ok).is_ok(), "{ok}");
        }
        for bad in [
            "https://127.0.0.1:49152/callback",
            "http://evil.example:49152/callback",
            "http://127.0.0.1.evil.example:80/x",
            "http://127.0.0.1/callback",
            "http://127.0.0.1:80/callback",
            "http://127.0.0.1:49152/cb?x=1",
            "http://user@127.0.0.1:49152/cb",
            "http://127.0.0.1:49152",
        ] {
            assert!(check_loopback_redirect(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn device_names() {
        assert_eq!(device_name("  ").unwrap(), "Desktop");
        assert_eq!(device_name(" PC\n1 ").unwrap(), "PC1");
        assert!(device_name(&"x".repeat(65)).is_err());
    }
}
