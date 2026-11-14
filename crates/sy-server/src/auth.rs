// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Login, browser sessions and request authentication (docs/ARCHITECTURE.md §9.3).
//!
//! # Flow
//!
//! 1. `POST /auth/login {username, password}`. Without a second factor the
//!    answer sets the session cookie. With TOTP it returns a short-lived
//!    challenge token instead.
//! 2. `POST /auth/login/second-factor {challenge, code}` with a TOTP code or
//!    a recovery code sets the session cookie.
//!
//! # Protection
//!
//! - Session cookie `sy_session`: random token, `HttpOnly`, `SameSite=Lax`,
//!   `Secure` on HTTPS; the database stores only its hash. Sliding expiry of
//!   [`SESSION_DAYS`].
//! - CSRF: requests that change something must send the session's CSRF
//!   token in `X-CSRF-Token` (from `GET /auth/status`). Login and
//!   registration (no session yet) check the `Origin` header instead.
//! - Failed attempts are counted per account and per client address
//!   ([`crate::ratelimit`]); error messages don't tell whether the account
//!   exists.

use std::net::SocketAddr;

use axum::Json;
use axum::extract::{ConnectInfo, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, StatusCode};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use deadpool_postgres::GenericClient;
use serde::{Deserialize, Serialize};
use tokio_postgres::Row;
use uuid::Uuid;

use crate::crypto;
use crate::error::{ApiError, ApiResult};
use crate::ratelimit::{PER_ACCOUNT, PER_ADDRESS};
use crate::state::{Registration, ServerSettings, SharedState, audit};

/// Name of the session cookie.
pub const SESSION_COOKIE: &str = "sy_session";
/// Header with the CSRF token.
pub const CSRF_HEADER: &str = "x-csrf-token";
/// Sessions expire after this many days without use.
pub const SESSION_DAYS: i64 = 30;
/// A password-accepted login must finish the second factor within this.
pub const CHALLENGE_MINUTES: i64 = 5;
/// Wrong codes allowed per challenge.
const CHALLENGE_ATTEMPTS: i32 = 5;

/// A user account (without secrets).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    /// Id
    pub id: Uuid,
    /// Login name (lowercase)
    pub username: String,
    /// Name shown to others
    pub display_name: String,
    /// Server admin
    pub is_admin: bool,
    /// Account disabled by an admin
    pub disabled: bool,
    /// Must set a new password before doing anything else
    pub must_change_password: bool,
    /// TOTP is set up
    pub totp_enabled: bool,
    /// At least one passkey is registered
    pub has_passkey: bool,
}

/// Columns for [`User::from_row`], prefixed with `u.`.
pub const USER_COLUMNS: &str = "u.id, u.username, u.display_name, u.is_admin, u.disabled, \
     u.must_change_password, u.totp_enabled_at IS NOT NULL, \
     EXISTS (SELECT 1 FROM passkeys p WHERE p.user_id = u.id)";

impl User {
    /// Reads [`USER_COLUMNS`] starting at column `offset`.
    pub fn from_row(row: &Row, offset: usize) -> Self {
        Self {
            id: row.get(offset),
            username: row.get(offset + 1),
            display_name: row.get(offset + 2),
            is_admin: row.get(offset + 3),
            disabled: row.get(offset + 4),
            must_change_password: row.get(offset + 5),
            totp_enabled: row.get(offset + 6),
            has_passkey: row.get(offset + 7),
        }
    }

    /// TOTP or a passkey: what an enforced 2FA setting asks for.
    pub fn has_second_factor(&self) -> bool {
        self.totp_enabled || self.has_passkey
    }

    /// Loads a user by id.
    pub async fn by_id(db: &impl GenericClient, id: Uuid) -> ApiResult<Option<Self>> {
        Ok(db
            .query_opt(
                &format!("SELECT {USER_COLUMNS} FROM users u WHERE u.id = $1"),
                &[&id],
            )
            .await?
            .map(|r| Self::from_row(&r, 0)))
    }
}

pub use sy_model::remote::Me;

/// The logged-in user as the frontend sees it, under `settings`.
pub fn me(user: &User, settings: &ServerSettings) -> Me {
    Me {
        id: user.id,
        username: user.username.clone(),
        display_name: user.display_name.clone(),
        is_admin: user.is_admin,
        totp_enabled: user.totp_enabled,
        has_passkey: user.has_passkey,
        must_change_password: user.must_change_password,
        two_factor_required: settings.enforce_2fa && !user.has_second_factor(),
    }
}

/// Normalizes and checks a username: 3–32 characters `a-z 0-9 . _ -`,
/// starting with a letter or digit.
pub fn normalize_username(name: &str) -> ApiResult<String> {
    let name = name.trim().to_lowercase();
    let ok = (3..=32).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if ok {
        Ok(name)
    } else {
        Err(ApiError::bad_request(
            "usernames have 3–32 characters: letters, digits, '.', '_' or '-'",
        ))
    }
}

/// Creates an account. Fails with 409 if the username is taken.
pub async fn create_user(
    db: &impl GenericClient,
    username: &str,
    display_name: &str,
    password_hash: &str,
    is_admin: bool,
    must_change_password: bool,
) -> ApiResult<Uuid> {
    let id = Uuid::now_v7();
    let display = display_name.trim();
    let display = if display.is_empty() {
        username
    } else {
        display
    };
    let result = db
        .execute(
            "INSERT INTO users (id, username, display_name, password_hash, is_admin, must_change_password)
             VALUES ($1, $2, $3, $4, $5, $6)",
            &[&id, &username, &display, &password_hash, &is_admin, &must_change_password],
        )
        .await;
    match result {
        Ok(_) => Ok(id),
        Err(e) if e.code() == Some(&tokio_postgres::error::SqlState::UNIQUE_VIOLATION) => {
            Err(ApiError::conflict("this username is taken"))
        }
        Err(e) => Err(e.into()),
    }
}

// ----- request context --------------------------------------------------

/// The client's address (for rate limiting and the audit log).
#[derive(Debug, Clone)]
pub struct ClientIp(pub Option<String>);

impl FromRequestParts<SharedState> for ClientIp {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &SharedState) -> ApiResult<Self> {
        if state.config.trust_proxy
            && let Some(forwarded) = parts
                .headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(',').next())
        {
            return Ok(Self(Some(forwarded.trim().to_owned())));
        }
        Ok(Self(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0.ip().to_string()),
        ))
    }
}

impl ClientIp {
    /// The address, if known.
    pub fn as_deref(&self) -> Option<&str> {
        self.0.as_deref()
    }

    /// Key for [`crate::ratelimit::RateLimiter`].
    pub fn limiter_key(&self) -> String {
        format!("ip:{}", self.0.as_deref().unwrap_or("unknown"))
    }
}

/// Rejects cross-site requests to endpoints without a session (login,
/// registration): a present `Origin` header must be the server's.
#[derive(Debug, Clone, Copy)]
pub struct SameOrigin;

impl FromRequestParts<SharedState> for SameOrigin {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &SharedState) -> ApiResult<Self> {
        match parts.headers.get("origin").and_then(|v| v.to_str().ok()) {
            Some(origin) if origin != state.config.public_origin() => {
                tracing::warn!(origin, "cross-origin request rejected");
                Err(ApiError::forbidden(
                    "request from another site rejected (check STINTORY_PUBLIC_URL)",
                ))
            }
            _ => Ok(Self),
        }
    }
}

/// A valid browser session.
#[derive(Debug, Clone)]
pub struct Session {
    /// The user
    pub user: User,
    /// Hash of the session token (browser sessions; empty for devices)
    pub token_hash: Vec<u8>,
    /// CSRF token of the session (browser sessions; empty for devices)
    pub csrf_token: String,
    /// The desktop client making the request (device token), if any
    pub device_id: Option<Uuid>,
    /// True for a read-only token (MCP endpoint only)
    pub read_only: bool,
}

async fn load_session(
    headers: &HeaderMap,
    state: &SharedState,
) -> ApiResult<Option<(Session, time::OffsetDateTime)>> {
    let jar = CookieJar::from_headers(headers);
    let Some(token) = jar.get(SESSION_COOKIE).map(|c| c.value().to_owned()) else {
        return Ok(None);
    };
    let hash = crypto::token_hash(&token);
    let db = state.conn().await?;
    let row = db
        .query_opt(
            &format!(
                "SELECT s.csrf_token, s.last_seen_at, {USER_COLUMNS}
                 FROM web_sessions s JOIN users u ON u.id = s.user_id
                 WHERE s.token_hash = $1 AND s.expires_at > now() AND NOT u.disabled"
            ),
            &[&hash],
        )
        .await?;
    Ok(row.map(|r| {
        (
            Session {
                user: User::from_row(&r, 2),
                token_hash: hash,
                csrf_token: r.get(0),
                device_id: None,
                read_only: false,
            },
            r.get(1),
        )
    }))
}

/// The token of an `Authorization: Bearer ...` header.
pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

/// Logged in, but possibly restricted (must change the password or set up
/// 2FA first). For the account endpoints that resolve the restriction.
#[derive(Debug, Clone)]
pub struct AccountAuth(pub Session);

impl FromRequestParts<SharedState> for AccountAuth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &SharedState) -> ApiResult<Self> {
        let Some((session, last_seen)) = load_session(&parts.headers, state).await? else {
            return Err(ApiError::unauthorized());
        };
        let safe = matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS);
        if !safe {
            let sent = parts.headers.get(CSRF_HEADER).and_then(|v| v.to_str().ok());
            if sent != Some(session.csrf_token.as_str()) {
                return Err(ApiError::forbidden("missing or wrong CSRF token").with_code("csrf"));
            }
        }
        // sliding expiry, written at most every few minutes
        if time::OffsetDateTime::now_utc() - last_seen > time::Duration::minutes(5) {
            state
                .conn()
                .await?
                .execute(
                    "UPDATE web_sessions SET last_seen_at = now(),
                        expires_at = now() + make_interval(days => $2)
                     WHERE token_hash = $1",
                    &[&session.token_hash, &(SESSION_DAYS as i32)],
                )
                .await?;
        }
        Ok(Self(session))
    }
}

/// Logged in with full access.
#[derive(Debug, Clone)]
pub struct Auth(pub Session);

impl FromRequestParts<SharedState> for Auth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &SharedState) -> ApiResult<Self> {
        let session = match bearer_token(&parts.headers).map(str::to_owned) {
            Some(token) => {
                let ip = ClientIp::from_request_parts(parts, state).await?;
                crate::devices::authenticate(state, &token, &ip).await?
            }
            None => AccountAuth::from_request_parts(parts, state).await?.0,
        };
        if session.read_only {
            return Err(
                ApiError::forbidden("this token only works for the MCP endpoint")
                    .with_code("read_only_token"),
            );
        }
        check_restrictions(state, &session).await?;
        Ok(Self(session))
    }
}

/// Rejects users who must change their password or set up 2FA first.
pub(crate) async fn check_restrictions(state: &SharedState, session: &Session) -> ApiResult<()> {
    if session.user.must_change_password {
        return Err(ApiError::forbidden("please set a new password first")
            .with_code("password_change_required"));
    }
    if !session.user.has_second_factor() {
        let settings = ServerSettings::load(&state.conn().await?).await?;
        if settings.enforce_2fa {
            return Err(
                ApiError::forbidden("please set up two-factor authentication first")
                    .with_code("two_factor_required"),
            );
        }
    }
    Ok(())
}

/// Logged in as server admin (full access).
#[derive(Debug, Clone)]
pub struct AdminAuth(pub Session);

impl FromRequestParts<SharedState> for AdminAuth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &SharedState) -> ApiResult<Self> {
        let Auth(session) = Auth::from_request_parts(parts, state).await?;
        if !session.user.is_admin || session.device_id.is_some() {
            return Err(ApiError::forbidden("admins only (in the browser)"));
        }
        Ok(Self(session))
    }
}

// ----- sessions ---------------------------------------------------------

fn session_cookie(state: &SharedState, token: String) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, token))
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(state.config.is_https())
        .path("/")
        .max_age(time::Duration::days(SESSION_DAYS))
        .build()
}

/// Starts a browser session for `user`; returns the cookie and the CSRF
/// token.
async fn start_session(
    state: &SharedState,
    user: &User,
    ip: &ClientIp,
    headers: &HeaderMap,
) -> ApiResult<(Cookie<'static>, String)> {
    let token = crypto::new_token();
    let csrf = crypto::new_token();
    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(200).collect::<String>());
    state
        .conn()
        .await?
        .execute(
            "INSERT INTO web_sessions (token_hash, user_id, csrf_token, expires_at, user_agent, ip)
             VALUES ($1, $2, $3, now() + make_interval(days => $4), $5, $6)",
            &[
                &crypto::token_hash(&token),
                &user.id,
                &csrf,
                &(SESSION_DAYS as i32),
                &user_agent,
                &ip.0,
            ],
        )
        .await?;
    Ok((session_cookie(state, token), csrf))
}

// ----- handlers ---------------------------------------------------------

/// `GET /auth/status`
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    /// The logged-in user, if any
    pub user: Option<Me>,
    /// CSRF token to send with changing requests
    pub csrf_token: Option<String>,
    /// Anyone may register
    pub registration_open: bool,
}

/// Who is logged in (never fails with 401).
pub async fn status(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> ApiResult<Json<AuthStatus>> {
    let settings = ServerSettings::load(&state.conn().await?).await?;
    let session = load_session(&headers, &state).await?;
    Ok(Json(AuthStatus {
        user: session.as_ref().map(|(s, _)| me(&s.user, &settings)),
        csrf_token: session.map(|(s, _)| s.csrf_token),
        registration_open: settings.registration == Registration::Open,
    }))
}

/// `POST /auth/login`
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    /// Username
    pub username: String,
    /// Password
    pub password: String,
}

/// Answer to a login step.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LoginResponse {
    /// Logged in
    Ok {
        /// The user
        user: Me,
        /// CSRF token of the new session
        csrf_token: String,
    },
    /// Password accepted, a second factor is needed
    SecondFactor {
        /// Pass to `/auth/login/second-factor`
        challenge: String,
        /// Accepted methods
        methods: Vec<String>,
    },
}

const WRONG_LOGIN: &str = "wrong username or password";

/// Checks username and password.
pub async fn login(
    State(state): State<SharedState>,
    ip: ClientIp,
    _: SameOrigin,
    headers: HeaderMap,
    jar: CookieJar,
    Json(req): Json<LoginRequest>,
) -> ApiResult<(CookieJar, Json<LoginResponse>)> {
    let username = req.username.trim().to_lowercase();
    let user_key = format!("user:{username}");
    if !state.limiter.allowed(&user_key, PER_ACCOUNT)
        || !state.limiter.allowed(&ip.limiter_key(), PER_ADDRESS)
    {
        audit(
            &state.conn().await?,
            None,
            "login_rate_limited",
            ip.as_deref(),
            serde_json::json!({ "username": username }),
        )
        .await;
        return Err(ApiError::too_many_requests());
    }
    let db = state.conn().await?;
    let row = db
        .query_opt(
            &format!("SELECT u.password_hash, {USER_COLUMNS} FROM users u WHERE u.username = $1"),
            &[&username],
        )
        .await?;
    let (hash, user) = match &row {
        Some(r) => (Some(r.get::<_, String>(0)), Some(User::from_row(r, 1))),
        None => (None, None),
    };
    let password_ok = crypto::verify_password(req.password, hash).await?;
    let user = match user {
        Some(u) if password_ok && !u.disabled => u,
        other => {
            state.limiter.fail(&user_key);
            state.limiter.fail(&ip.limiter_key());
            audit(
                &db,
                other.as_ref().map(|u| u.id),
                "login_failed",
                ip.as_deref(),
                serde_json::json!({
                    "username": username,
                    "reason": match other {
                        None => "unknown user",
                        Some(ref u) if u.disabled && password_ok => "disabled",
                        Some(_) => "wrong password",
                    }
                }),
            )
            .await;
            return Err(ApiError::new(StatusCode::UNAUTHORIZED, WRONG_LOGIN));
        }
    };

    if user.has_second_factor() {
        let challenge = crypto::new_token();
        db.execute(
            "INSERT INTO login_challenges (token_hash, user_id, expires_at)
             VALUES ($1, $2, now() + make_interval(mins => $3))",
            &[
                &crypto::token_hash(&challenge),
                &user.id,
                &(CHALLENGE_MINUTES as i32),
            ],
        )
        .await?;
        return Ok((
            jar,
            Json(LoginResponse::SecondFactor {
                challenge,
                methods: [("totp", user.totp_enabled), ("passkey", user.has_passkey)]
                    .into_iter()
                    .filter(|(_, on)| *on)
                    .map(|(m, _)| m.to_owned())
                    .collect(),
            }),
        ));
    }
    finish_login(&state, user, &ip, &headers, jar, "password").await
}

pub(crate) async fn finish_login(
    state: &SharedState,
    user: User,
    ip: &ClientIp,
    headers: &HeaderMap,
    jar: CookieJar,
    method: &str,
) -> ApiResult<(CookieJar, Json<LoginResponse>)> {
    state.limiter.reset(&format!("user:{}", user.username));
    let (cookie, csrf) = start_session(state, &user, ip, headers).await?;
    let db = state.conn().await?;
    audit(
        &db,
        Some(user.id),
        "login",
        ip.as_deref(),
        serde_json::json!({ "method": method }),
    )
    .await;
    let settings = ServerSettings::load(&db).await?;
    Ok((
        jar.add(cookie),
        Json(LoginResponse::Ok {
            user: me(&user, &settings),
            csrf_token: csrf,
        }),
    ))
}

/// `POST /auth/login/second-factor`
#[derive(Debug, Deserialize)]
pub struct SecondFactorRequest {
    /// From the login answer
    pub challenge: String,
    /// TOTP code or recovery code
    pub code: String,
}

/// Finishes a login with a TOTP or recovery code.
pub async fn second_factor(
    State(state): State<SharedState>,
    ip: ClientIp,
    _: SameOrigin,
    headers: HeaderMap,
    jar: CookieJar,
    Json(req): Json<SecondFactorRequest>,
) -> ApiResult<(CookieJar, Json<LoginResponse>)> {
    let expired = || {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "the login has expired, please start again",
        )
        .with_code("challenge_expired")
    };
    if !state.limiter.allowed(&ip.limiter_key(), PER_ADDRESS) {
        return Err(ApiError::too_many_requests());
    }
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    let challenge_hash = crypto::token_hash(&req.challenge);
    let Some(row) = tx
        .query_opt(
            &format!(
                "SELECT c.attempts, u.totp_secret, u.totp_last_step, {USER_COLUMNS}
                 FROM login_challenges c JOIN users u ON u.id = c.user_id
                 WHERE c.token_hash = $1 AND c.expires_at > now() AND NOT u.disabled
                 FOR UPDATE OF c, u"
            ),
            &[&challenge_hash],
        )
        .await?
    else {
        return Err(expired());
    };
    let attempts: i32 = row.get(0);
    let user = User::from_row(&row, 3);
    if attempts >= CHALLENGE_ATTEMPTS
        || !state
            .limiter
            .allowed(&format!("user:{}", user.username), PER_ACCOUNT)
    {
        tx.execute(
            "DELETE FROM login_challenges WHERE token_hash = $1",
            &[&challenge_hash],
        )
        .await?;
        tx.commit().await?;
        return Err(expired());
    }

    let secret: Option<Vec<u8>> = row.get(1);
    let last_step: Option<i64> = row.get(2);
    let mut method = None;
    if let Some(sealed) = secret {
        let secret = state.secrets.open(user.id, &sealed)?;
        let last = last_step.and_then(|s| u64::try_from(s).ok());
        if let Some(step) = crypto::totp_verify(&secret, &req.code, crypto::totp_step_now(), last) {
            tx.execute(
                "UPDATE users SET totp_last_step = $2 WHERE id = $1",
                &[&user.id, &(step as i64)],
            )
            .await?;
            method = Some("totp");
        }
    }
    if method.is_none() {
        let code_hash = state
            .secrets
            .mac(crypto::normalize_recovery_code(&req.code).as_bytes());
        let used = tx
            .execute(
                "UPDATE recovery_codes SET used_at = now()
                 WHERE user_id = $1 AND code_hash = $2 AND used_at IS NULL",
                &[&user.id, &code_hash],
            )
            .await?;
        if used > 0 {
            method = Some("recovery_code");
        }
    }

    let Some(method) = method else {
        tx.execute(
            "UPDATE login_challenges SET attempts = attempts + 1 WHERE token_hash = $1",
            &[&challenge_hash],
        )
        .await?;
        tx.commit().await?;
        state.limiter.fail(&format!("user:{}", user.username));
        state.limiter.fail(&ip.limiter_key());
        audit(
            &state.conn().await?,
            Some(user.id),
            "login_failed",
            ip.as_deref(),
            serde_json::json!({ "reason": "wrong second factor" }),
        )
        .await;
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "wrong code").with_code("wrong_code"));
    };
    tx.execute(
        "DELETE FROM login_challenges WHERE token_hash = $1",
        &[&challenge_hash],
    )
    .await?;
    tx.commit().await?;
    finish_login(&state, user, &ip, &headers, jar, method).await
}

/// `POST /auth/logout`: ends the current session.
pub async fn logout(
    State(state): State<SharedState>,
    AccountAuth(session): AccountAuth,
    ip: ClientIp,
    jar: CookieJar,
) -> ApiResult<(CookieJar, StatusCode)> {
    let db = state.conn().await?;
    db.execute(
        "DELETE FROM web_sessions WHERE token_hash = $1",
        &[&session.token_hash],
    )
    .await?;
    audit(
        &db,
        Some(session.user.id),
        "logout",
        ip.as_deref(),
        serde_json::json!({}),
    )
    .await;
    Ok((
        jar.remove(Cookie::build(SESSION_COOKIE).path("/")),
        StatusCode::NO_CONTENT,
    ))
}

/// `POST /auth/register`
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterRequest {
    /// Username
    pub username: String,
    /// Password
    pub password: String,
    /// Name shown to others (defaults to the username)
    #[serde(default)]
    pub display_name: String,
}

/// Creates an account when registration is open, and logs it in.
pub async fn register(
    State(state): State<SharedState>,
    ip: ClientIp,
    _: SameOrigin,
    headers: HeaderMap,
    jar: CookieJar,
    Json(req): Json<RegisterRequest>,
) -> ApiResult<(CookieJar, Json<LoginResponse>)> {
    let db = state.conn().await?;
    if ServerSettings::load(&db).await?.registration != Registration::Open {
        return Err(ApiError::forbidden("registration is closed on this server"));
    }
    if !state.limiter.allowed(&ip.limiter_key(), PER_ADDRESS) {
        return Err(ApiError::too_many_requests());
    }
    let username = normalize_username(&req.username)?;
    crypto::check_password_policy(&req.password)?;
    let hash = crypto::hash_password(req.password).await?;
    let id = create_user(&db, &username, &req.display_name, &hash, false, false).await?;
    // registrations count like failures: slows down mass sign-ups
    state.limiter.fail(&ip.limiter_key());
    audit(
        &db,
        Some(id),
        "registered",
        ip.as_deref(),
        serde_json::json!({}),
    )
    .await;
    let user = User::by_id(&db, id)
        .await?
        .ok_or_else(|| ApiError::internal("user vanished"))?;
    finish_login(&state, user, &ip, &headers, jar, "registration").await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames() {
        assert_eq!(normalize_username(" Andi.B ").unwrap(), "andi.b");
        assert!(normalize_username("ab").is_err());
        assert!(normalize_username("_x_y").is_err());
        assert!(normalize_username("a b c").is_err());
        assert!(normalize_username("äbc").is_err());
        assert!(normalize_username(&"a".repeat(33)).is_err());
    }
}
