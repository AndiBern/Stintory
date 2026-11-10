// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Passkeys (WebAuthn, docs/PLAN.md §9.3), via `webauthn-rs`.
//!
//! - **Passwordless login**: `POST /auth/passkey/start` + `…/finish`. The
//!   browser offers the user's passkeys for this site (discoverable
//!   credentials); no username needed. User verification (PIN, biometrics)
//!   is required, so this counts as two factors.
//! - **Second factor** after the password: `POST
//!   /auth/login/second-factor/passkey/start` + `…/finish`.
//! - **Managing**: `/me/passkeys` (adding needs the account password).
//!
//! The relying party id is the host of `RZ_PUBLIC_URL`; passkeys only work
//! on that address (and need HTTPS, except on `localhost`). Ceremony state is
//! kept server-side in `webauthn_challenges`, single use, for
//! [`CEREMONY_SECONDS`].

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum_extra::extract::cookie::CookieJar;
use deadpool_postgres::GenericClient;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use webauthn_rs::prelude::{
    CreationChallengeResponse, DiscoverableAuthentication, Passkey, PasskeyAuthentication,
    PasskeyRegistration, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse, Url, Webauthn, WebauthnBuilder,
};

use crate::auth::{
    AccountAuth, ClientIp, LoginResponse, SameOrigin, USER_COLUMNS, User, finish_login,
};
use crate::config::Config;
use crate::crypto;
use crate::error::{ApiError, ApiResult};
use crate::ratelimit::PER_ADDRESS;
use crate::state::{ServerSettings, SharedState, audit};

/// How long a started ceremony stays valid.
pub const CEREMONY_SECONDS: i32 = 300;

/// The WebAuthn relying party for the server's public address.
pub fn relying_party(config: &Config) -> Result<Webauthn, String> {
    let origin = Url::parse(&config.public_origin())
        .map_err(|e| format!("RZ_PUBLIC_URL is not a valid URL: {e}"))?;
    let rp_id = origin
        .host_str()
        .ok_or("RZ_PUBLIC_URL has no host")?
        .to_owned();
    WebauthnBuilder::new(&rp_id, &origin)
        .and_then(|b| b.rp_name("Rundenzeit").build())
        .map_err(|e| format!("passkeys can't be used with RZ_PUBLIC_URL {origin}: {e}"))
}

fn webauthn(state: &SharedState) -> ApiResult<&Webauthn> {
    state.webauthn.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "passkeys are not available on this server (check RZ_PUBLIC_URL)",
        )
    })
}

fn webauthn_error(e: impl std::fmt::Display) -> ApiError {
    tracing::info!(error = %e, "passkey ceremony failed");
    ApiError::bad_request("the passkey could not be verified").with_code("passkey_failed")
}

// ----- ceremony state ------------------------------------------------------

async fn save_ceremony<T: Serialize>(
    db: &impl GenericClient,
    user: Option<Uuid>,
    purpose: &str,
    state: &T,
    extra: serde_json::Value,
) -> ApiResult<String> {
    let id = crypto::new_token();
    db.execute(
        "DELETE FROM webauthn_challenges WHERE expires_at < now()",
        &[],
    )
    .await?;
    db.execute(
        "INSERT INTO webauthn_challenges (token_hash, user_id, purpose, state, extra, expires_at)
         VALUES ($1, $2, $3, $4, $5, now() + make_interval(secs => $6))",
        &[
            &crypto::token_hash(&id),
            &user,
            &purpose,
            &serde_json::to_value(state)?,
            &extra,
            &f64::from(CEREMONY_SECONDS),
        ],
    )
    .await?;
    Ok(id)
}

/// Takes (deletes) a ceremony; it can't be used twice.
async fn take_ceremony<T: DeserializeOwned>(
    db: &impl GenericClient,
    id: &str,
    purpose: &str,
) -> ApiResult<(Option<Uuid>, T, serde_json::Value)> {
    let row = db
        .query_opt(
            "DELETE FROM webauthn_challenges
             WHERE token_hash = $1 AND purpose = $2 AND expires_at > now()
             RETURNING user_id, state, extra",
            &[&crypto::token_hash(id), &purpose],
        )
        .await?
        .ok_or_else(|| {
            ApiError::bad_request("the passkey request has expired, please try again")
                .with_code("passkey_expired")
        })?;
    Ok((row.get(0), serde_json::from_value(row.get(1))?, row.get(2)))
}

async fn user_passkeys(db: &impl GenericClient, user: Uuid) -> ApiResult<Vec<(Uuid, Passkey)>> {
    db.query(
        "SELECT id, passkey FROM passkeys WHERE user_id = $1 ORDER BY created_at",
        &[&user],
    )
    .await?
    .iter()
    .map(|r| Ok((r.get(0), serde_json::from_value(r.get(1))?)))
    .collect()
}

/// Stores the new signature counter (and backup state) after a login.
async fn record_use(
    db: &impl GenericClient,
    id: Uuid,
    mut passkey: Passkey,
    result: &webauthn_rs::prelude::AuthenticationResult,
) -> ApiResult<()> {
    passkey.update_credential(result);
    db.execute(
        "UPDATE passkeys SET passkey = $2, last_used_at = now() WHERE id = $1",
        &[&id, &serde_json::to_value(&passkey)?],
    )
    .await?;
    Ok(())
}

/// A started ceremony: pass `options` to the browser's WebAuthn API and
/// `challengeId` back with the result.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ceremony<T> {
    /// Id of the server-side state
    pub challenge_id: String,
    /// `navigator.credentials` options
    pub options: T,
}

// ----- managing passkeys ----------------------------------------------------

/// A registered passkey.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasskeyInfo {
    /// Id
    pub id: Uuid,
    /// Name given by the user, e.g. "Phone"
    pub name: String,
    /// Added
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    /// Last login with it
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<time::OffsetDateTime>,
}

/// `GET /me/passkeys`
pub async fn list(
    State(state): State<SharedState>,
    AccountAuth(session): AccountAuth,
) -> ApiResult<Json<Vec<PasskeyInfo>>> {
    let rows = state
        .conn()
        .await?
        .query(
            "SELECT id, name, created_at, last_used_at FROM passkeys
             WHERE user_id = $1 ORDER BY created_at",
            &[&session.user.id],
        )
        .await?;
    Ok(Json(
        rows.iter()
            .map(|r| PasskeyInfo {
                id: r.get(0),
                name: r.get(1),
                created_at: r.get(2),
                last_used_at: r.get(3),
            })
            .collect(),
    ))
}

/// `POST /me/passkeys/register/start`
#[derive(Debug, Deserialize)]
pub struct RegisterStart {
    /// Account password, to confirm
    pub password: String,
    /// Name for the list
    #[serde(default)]
    pub name: String,
}

fn passkey_name(name: &str) -> ApiResult<String> {
    let name: String = name.trim().chars().filter(|c| !c.is_control()).collect();
    match name.chars().count() {
        0 => Ok("Passkey".into()),
        1..=64 => Ok(name),
        _ => Err(ApiError::bad_request("the name is too long")),
    }
}

/// Starts adding a passkey.
pub async fn register_start(
    State(state): State<SharedState>,
    AccountAuth(session): AccountAuth,
    Json(req): Json<RegisterStart>,
) -> ApiResult<Json<Ceremony<CreationChallengeResponse>>> {
    let user = &session.user;
    let name = passkey_name(&req.name)?;
    let hash: String = state
        .conn()
        .await?
        .query_one("SELECT password_hash FROM users WHERE id = $1", &[&user.id])
        .await?
        .get(0);
    if !crypto::verify_password(req.password, Some(hash)).await? {
        return Err(ApiError::forbidden("wrong password").with_code("wrong_password"));
    }
    let db = state.conn().await?;
    let existing = user_passkeys(&db, user.id).await?;
    let exclude = existing.iter().map(|(_, p)| p.cred_id().clone()).collect();
    let (mut options, reg) = webauthn(&state)?
        .start_passkey_registration(user.id, &user.username, &user.display_name, Some(exclude))
        .map_err(webauthn_error)?;
    // ask for a discoverable credential, so it also works without username
    if let Some(sel) = options.public_key.authenticator_selection.as_mut() {
        sel.resident_key = Some(webauthn_rs_proto::ResidentKeyRequirement::Preferred);
    }
    let id = save_ceremony::<PasskeyRegistration>(
        &db,
        Some(user.id),
        "register",
        &reg,
        serde_json::json!({ "name": name }),
    )
    .await?;
    Ok(Json(Ceremony {
        challenge_id: id,
        options,
    }))
}

/// `POST /me/passkeys/register/finish`
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterFinish {
    /// From the start
    pub challenge_id: String,
    /// The browser's `navigator.credentials.create()` result
    pub credential: RegisterPublicKeyCredential,
}

/// Stores the new passkey.
pub async fn register_finish(
    State(state): State<SharedState>,
    AccountAuth(session): AccountAuth,
    ip: ClientIp,
    Json(req): Json<RegisterFinish>,
) -> ApiResult<Json<PasskeyInfo>> {
    let db = state.conn().await?;
    let (user, reg, extra): (_, PasskeyRegistration, _) =
        take_ceremony(&db, &req.challenge_id, "register").await?;
    if user != Some(session.user.id) {
        return Err(ApiError::bad_request(
            "the passkey request belongs to another login",
        ));
    }
    let passkey = webauthn(&state)?
        .finish_passkey_registration(&req.credential, &reg)
        .map_err(webauthn_error)?;
    let id = Uuid::now_v7();
    let name = extra["name"].as_str().unwrap_or("Passkey").to_owned();
    let inserted = db
        .execute(
            "INSERT INTO passkeys (id, user_id, credential_id, name, passkey)
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT (credential_id) DO NOTHING",
            &[
                &id,
                &session.user.id,
                &passkey.cred_id().as_slice(),
                &name,
                &serde_json::to_value(&passkey)?,
            ],
        )
        .await?;
    if inserted == 0 {
        return Err(ApiError::conflict("this passkey is already registered"));
    }
    audit(
        &db,
        Some(session.user.id),
        "passkey_added",
        ip.as_deref(),
        serde_json::json!({ "passkey": id, "name": name }),
    )
    .await;
    Ok(Json(PasskeyInfo {
        id,
        name,
        created_at: time::OffsetDateTime::now_utc(),
        last_used_at: None,
    }))
}

/// `DELETE /me/passkeys/{id}`
pub async fn remove(
    State(state): State<SharedState>,
    AccountAuth(session): AccountAuth,
    ip: ClientIp,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let user = &session.user;
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    let n = tx
        .execute(
            "DELETE FROM passkeys WHERE id = $1 AND user_id = $2",
            &[&id, &user.id],
        )
        .await?;
    if n == 0 {
        return Err(ApiError::not_found("passkey"));
    }
    let left: i64 = tx
        .query_one(
            "SELECT count(*) FROM passkeys WHERE user_id = $1",
            &[&user.id],
        )
        .await?
        .get(0);
    if left == 0 && !user.totp_enabled && ServerSettings::load(&tx).await?.enforce_2fa {
        return Err(ApiError::forbidden(
            "two-factor authentication is required on this server: set up TOTP or another passkey first",
        ));
    }
    tx.commit().await?;
    audit(
        &db,
        Some(user.id),
        "passkey_removed",
        ip.as_deref(),
        serde_json::json!({ "passkey": id }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// ----- passwordless login ----------------------------------------------------

/// `POST /auth/passkey/start`: options for "log in with a passkey".
pub async fn login_start(
    State(state): State<SharedState>,
    ip: ClientIp,
    _: SameOrigin,
) -> ApiResult<Json<Ceremony<RequestChallengeResponse>>> {
    if !state.limiter.allowed(&ip.limiter_key(), PER_ADDRESS) {
        return Err(ApiError::too_many_requests());
    }
    let (mut options, auth) = webauthn(&state)?
        .start_discoverable_authentication()
        .map_err(webauthn_error)?;
    // a button press, not the autofill ("conditional") UI
    options.mediation = None;
    let id = save_ceremony::<DiscoverableAuthentication>(
        &state.conn().await?,
        None,
        "login",
        &auth,
        serde_json::json!({}),
    )
    .await?;
    Ok(Json(Ceremony {
        challenge_id: id,
        options,
    }))
}

/// `POST /auth/passkey/finish`, `…/second-factor/passkey/finish`
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginFinish {
    /// From the start
    pub challenge_id: String,
    /// The browser's `navigator.credentials.get()` result
    pub credential: PublicKeyCredential,
    /// Second factor only: the challenge from the password step
    #[serde(default)]
    pub challenge: Option<String>,
}

/// Logs in with a passkey alone.
pub async fn login_finish(
    State(state): State<SharedState>,
    ip: ClientIp,
    _: SameOrigin,
    headers: HeaderMap,
    jar: CookieJar,
    Json(req): Json<LoginFinish>,
) -> ApiResult<(CookieJar, Json<LoginResponse>)> {
    let db = state.conn().await?;
    let fail = |reason: &'static str| {
        state.limiter.fail(&ip.limiter_key());
        let state = state.clone();
        let ip = ip.0.clone();
        async move {
            if let Ok(db) = state.conn().await {
                audit(
                    &db,
                    None,
                    "login_failed",
                    ip.as_deref(),
                    serde_json::json!({ "method": "passkey", "reason": reason }),
                )
                .await;
            }
            ApiError::new(StatusCode::UNAUTHORIZED, "this passkey is not known here")
                .with_code("passkey_failed")
        }
    };
    let (_, auth, _): (_, DiscoverableAuthentication, _) =
        take_ceremony(&db, &req.challenge_id, "login").await?;
    // find the passkey by its credential id; the user handle (if sent) must match
    let row = db
        .query_opt(
            &format!(
                "SELECT p.id, p.passkey, {USER_COLUMNS}
                 FROM passkeys p JOIN users u ON u.id = p.user_id
                 WHERE p.credential_id = $1"
            ),
            &[&req.credential.get_credential_id()],
        )
        .await?;
    let Some(row) = row else {
        return Err(fail("unknown passkey").await);
    };
    let (passkey_id, passkey): (Uuid, Passkey) = (row.get(0), serde_json::from_value(row.get(1))?);
    let user = User::from_row(&row, 2);
    if let Some(handle) = req.credential.get_user_unique_id()
        && handle != user.id.as_bytes()
    {
        return Err(fail("user handle mismatch").await);
    }
    if user.disabled {
        return Err(fail("disabled").await);
    }
    let result = match webauthn(&state)?.finish_discoverable_authentication_v2(
        &req.credential,
        auth,
        &passkey,
    ) {
        Ok(r) => r,
        Err(e) => {
            tracing::info!(error = %e, "passkey login failed");
            return Err(fail("verification failed").await);
        }
    };
    record_use(&db, passkey_id, passkey, &result).await?;
    finish_login(&state, user, &ip, &headers, jar, "passkey").await
}

// ----- second factor ---------------------------------------------------------

/// `POST /auth/login/second-factor/passkey/start`
#[derive(Debug, Deserialize)]
pub struct SecondFactorStart {
    /// The challenge from the password step
    pub challenge: String,
}

async fn login_challenge_user(db: &impl GenericClient, challenge: &str) -> ApiResult<User> {
    db.query_opt(
        &format!(
            "SELECT {USER_COLUMNS} FROM login_challenges c JOIN users u ON u.id = c.user_id
             WHERE c.token_hash = $1 AND c.expires_at > now() AND c.attempts < 5
               AND NOT u.disabled"
        ),
        &[&crypto::token_hash(challenge)],
    )
    .await?
    .map(|r| User::from_row(&r, 0))
    .ok_or_else(|| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "the login has expired, please start again",
        )
        .with_code("challenge_expired")
    })
}

/// Options to confirm a password login with one of the user's passkeys.
pub async fn second_factor_start(
    State(state): State<SharedState>,
    _: SameOrigin,
    Json(req): Json<SecondFactorStart>,
) -> ApiResult<Json<Ceremony<RequestChallengeResponse>>> {
    let db = state.conn().await?;
    let user = login_challenge_user(&db, &req.challenge).await?;
    let passkeys: Vec<Passkey> = user_passkeys(&db, user.id)
        .await?
        .into_iter()
        .map(|(_, p)| p)
        .collect();
    if passkeys.is_empty() {
        return Err(ApiError::bad_request("no passkey registered"));
    }
    let (options, auth) = webauthn(&state)?
        .start_passkey_authentication(&passkeys)
        .map_err(webauthn_error)?;
    let id = save_ceremony::<PasskeyAuthentication>(
        &db,
        Some(user.id),
        "second_factor",
        &auth,
        serde_json::json!({ "challenge": hex::encode(crypto::token_hash(&req.challenge)) }),
    )
    .await?;
    Ok(Json(Ceremony {
        challenge_id: id,
        options,
    }))
}

/// Finishes a password login with a passkey.
pub async fn second_factor_finish(
    State(state): State<SharedState>,
    ip: ClientIp,
    _: SameOrigin,
    headers: HeaderMap,
    jar: CookieJar,
    Json(req): Json<LoginFinish>,
) -> ApiResult<(CookieJar, Json<LoginResponse>)> {
    let challenge = req
        .challenge
        .ok_or_else(|| ApiError::bad_request("challenge missing"))?;
    let db = state.conn().await?;
    let user = login_challenge_user(&db, &challenge).await?;
    let challenge_hash = crypto::token_hash(&challenge);
    let (owner, auth, extra): (_, PasskeyAuthentication, _) =
        take_ceremony(&db, &req.challenge_id, "second_factor").await?;
    if owner != Some(user.id) || extra["challenge"].as_str() != Some(&hex::encode(&challenge_hash))
    {
        return Err(ApiError::bad_request(
            "the passkey request belongs to another login",
        ));
    }
    let result = match webauthn(&state)?.finish_passkey_authentication(&req.credential, &auth) {
        Ok(r) => r,
        Err(e) => {
            tracing::info!(error = %e, "passkey second factor failed");
            db.execute(
                "UPDATE login_challenges SET attempts = attempts + 1 WHERE token_hash = $1",
                &[&challenge_hash],
            )
            .await?;
            state.limiter.fail(&format!("user:{}", user.username));
            audit(
                &db,
                Some(user.id),
                "login_failed",
                ip.as_deref(),
                serde_json::json!({ "reason": "passkey second factor" }),
            )
            .await;
            return Err(webauthn_error(e));
        }
    };
    let used = user_passkeys(&db, user.id)
        .await?
        .into_iter()
        .find(|(_, p)| p.cred_id() == result.cred_id());
    if let Some((id, passkey)) = used {
        record_use(&db, id, passkey, &result).await?;
    }
    db.execute(
        "DELETE FROM login_challenges WHERE token_hash = $1",
        &[&challenge_hash],
    )
    .await?;
    finish_login(&state, user, &ip, &headers, jar, "password+passkey").await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relying_party_from_public_url() {
        let mut c = Config::for_tests(String::new(), std::path::PathBuf::new());
        c.public_url = "https://laps.example.com/".into();
        assert!(relying_party(&c).is_ok());
        c.public_url = "http://localhost:8080".into();
        assert!(relying_party(&c).is_ok());
    }

    #[test]
    fn names() {
        assert_eq!(passkey_name(" ").unwrap(), "Passkey");
        assert_eq!(passkey_name(" Phone ").unwrap(), "Phone");
        assert!(passkey_name(&"x".repeat(65)).is_err());
    }
}
