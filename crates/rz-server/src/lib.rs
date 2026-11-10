// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Rundenzeit server (docs/PLAN.md §9): accounts, the web UI and (later)
//! sync and sharing.
//!
//! Routes: the JSON API under `/api/v1`, everything else is the web UI
//! (`web/build`, a single-page app).

pub mod account;
pub mod admin;
pub mod auth;
pub mod config;
pub mod crypto;
pub mod data;
pub mod db;
pub mod devices;
pub mod error;
pub mod passkeys;
pub mod ratelimit;
pub mod state;
pub mod sync;

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderName, HeaderValue};
use axum::routing::{delete, get, patch, post};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

use crate::config::Config;
use crate::crypto::SecretBox;
use crate::error::{ApiError, ApiResult};
use crate::state::{AppState, SharedState, audit};

/// Opens the database (migrating it), the blob store, and creates the first
/// admin if configured. `search_path` selects a schema (tests).
pub async fn init(config: Config, search_path: Option<&str>) -> ApiResult<SharedState> {
    config
        .validate()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let db = db::pool(&config.database_url, search_path)?;
    db::wait_until_reachable(
        &db,
        &db::describe(&config.database_url),
        std::time::Duration::from_secs(config.db_wait_seconds),
    )
    .await?;
    db::migrate(&db).await?;
    let blobs = rz_store::BlobStore::open(config.data_dir.join("blobs"))?;
    let secrets = SecretBox::new(
        config
            .secret_key_bytes()
            .map_err(|e| ApiError::internal(e.to_string()))?,
    );
    let webauthn = passkeys::relying_party(&config)
        .map_err(|e| tracing::warn!("{e}; passkeys are disabled"))
        .ok();
    let state = Arc::new(AppState {
        config,
        db,
        blobs,
        secrets,
        limiter: Default::default(),
        webauthn,
    });
    bootstrap_admin(&state).await?;
    Ok(state)
}

/// Creates the admin from `RZ_ADMIN_USERNAME`/`RZ_ADMIN_PASSWORD` if the
/// server has no users yet.
async fn bootstrap_admin(state: &SharedState) -> ApiResult<()> {
    let (Some(name), Some(password)) = (
        state.config.admin_username.as_deref(),
        state.config.admin_password.as_deref(),
    ) else {
        return Ok(());
    };
    let db = state.conn().await?;
    let users: i64 = db
        .query_one("SELECT count(*) FROM users", &[])
        .await?
        .get(0);
    if users > 0 {
        return Ok(());
    }
    let username = auth::normalize_username(name)?;
    crypto::check_password_policy(password)
        .map_err(|e| ApiError::internal(format!("RZ_ADMIN_PASSWORD: {}", e.message())))?;
    let hash = crypto::hash_password(password.to_owned()).await?;
    let id = auth::create_user(&db, &username, "", &hash, true, false).await?;
    audit(
        &db,
        Some(id),
        "admin_bootstrapped",
        None,
        serde_json::json!({ "username": username }),
    )
    .await;
    tracing::info!(username, "created the first admin account");
    Ok(())
}

/// The JSON API (`/api/v1`).
fn api(state: &SharedState) -> Router<SharedState> {
    let upload_limit =
        usize::try_from(state.config.max_upload_mb * 1024 * 1024).unwrap_or(usize::MAX);
    Router::new()
        .route("/info", get(data::info))
        // auth
        .route("/auth/status", get(auth::status))
        .route("/auth/login", post(auth::login))
        .route("/auth/login/second-factor", post(auth::second_factor))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/register", post(auth::register))
        .route("/auth/passkey/start", post(passkeys::login_start))
        .route("/auth/passkey/finish", post(passkeys::login_finish))
        .route(
            "/auth/login/second-factor/passkey/start",
            post(passkeys::second_factor_start),
        )
        .route(
            "/auth/login/second-factor/passkey/finish",
            post(passkeys::second_factor_finish),
        )
        // account
        .route("/me", get(account::me).patch(account::update_me))
        .route("/me/password", post(account::change_password))
        .route("/me/totp/setup", post(account::totp_setup))
        .route("/me/totp/confirm", post(account::totp_confirm))
        .route("/me/totp", delete(account::totp_disable))
        .route(
            "/me/recovery-codes",
            post(account::regenerate_recovery_codes),
        )
        .route("/me/sessions", get(account::sessions))
        .route("/me/passkeys", get(passkeys::list))
        .route(
            "/me/passkeys/register/start",
            post(passkeys::register_start),
        )
        .route(
            "/me/passkeys/register/finish",
            post(passkeys::register_finish),
        )
        .route("/me/passkeys/{id}", delete(passkeys::remove))
        .route("/me/sessions/{id}", delete(account::end_session))
        // admin
        .route("/admin/users", get(admin::list_users).post(admin::create))
        .route("/admin/users/{id}", patch(admin::update))
        .route(
            "/admin/settings",
            get(admin::get_settings).put(admin::put_settings),
        )
        .route("/admin/audit", get(admin::audit_log))
        // desktop clients
        .route("/device-login/approve", post(devices::approve))
        .route("/device-login/token", post(devices::token))
        .route(
            "/devices/current",
            get(devices::current).delete(devices::disconnect),
        )
        .route(
            "/me/devices",
            get(devices::my_devices).post(devices::create),
        )
        .route("/me/devices/{id}", delete(devices::revoke))
        // sync
        .route("/sync/blobs/missing", post(sync::missing_blobs))
        .route(
            "/sync/blobs/{hash}",
            get(sync::get_blob)
                .put(sync::put_blob)
                .layer(DefaultBodyLimit::max(upload_limit)),
        )
        .route(
            "/sync/push",
            post(sync::push).layer(DefaultBodyLimit::max(64 << 20)),
        )
        .route("/sync/changes", get(sync::changes))
        // data
        .route("/sessions", get(data::list_sessions))
        .route(
            "/sessions/{id}",
            get(data::get_session).delete(data::delete_session),
        )
        .route("/analysis/compare", post(data::compare))
        .route(
            "/import",
            post(data::import).layer(DefaultBodyLimit::max(upload_limit)),
        )
        .fallback(|| async { ApiError::not_found("API endpoint") })
}

/// The complete application.
pub fn app(state: SharedState) -> Router {
    let mut router = Router::new().nest("/api/v1", api(&state));
    let web = &state.config.web_dir;
    if web.join("index.html").is_file() {
        // single-page app: unknown paths get index.html
        router = router
            .fallback_service(ServeDir::new(web).fallback(ServeFile::new(web.join("index.html"))));
    } else {
        tracing::warn!(dir = %web.display(), "web UI not found, serving the API only");
    }
    let header = |name: &'static str, value: &'static str| {
        SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        )
    };
    router
        .layer(header("x-content-type-options", "nosniff"))
        .layer(header("referrer-policy", "same-origin"))
        .layer(header("x-frame-options", "DENY"))
        .layer(header(
            "content-security-policy",
            "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'self'; form-action 'self'",
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
