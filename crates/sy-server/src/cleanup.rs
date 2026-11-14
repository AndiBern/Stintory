// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Periodic housekeeping (spawned by `sy-server serve`): removes telemetry
//! files nothing uses ([`crate::sync::remove_orphan_blobs`]) and database
//! rows that have expired.

use std::time::Duration;

use deadpool_postgres::GenericClient;

use crate::error::ApiResult;
use crate::state::SharedState;

/// How often [`run_periodically`] runs.
const EVERY: Duration = Duration::from_secs(3600);

/// Deletes expired rows: browser sessions, unfinished logins and passkey
/// ceremonies, unused device-login and OAuth codes, OAuth access tokens and
/// OAuth grants whose refresh token expired. None of them can be used
/// anymore; most are otherwise only removed when a new one is created.
/// Returns the number of deleted rows.
pub async fn remove_expired(db: &impl GenericClient) -> ApiResult<u64> {
    let mut deleted = 0;
    for sql in [
        "DELETE FROM web_sessions WHERE expires_at < now()",
        "DELETE FROM login_challenges WHERE expires_at < now()",
        "DELETE FROM webauthn_challenges WHERE expires_at < now()",
        "DELETE FROM device_codes WHERE expires_at < now()",
        "DELETE FROM oauth_codes WHERE expires_at < now()",
        "DELETE FROM oauth_access_tokens WHERE expires_at < now()",
        "DELETE FROM device_tokens
         WHERE oauth_client IS NOT NULL AND refresh_expires_at < now()",
    ] {
        deleted += db.execute(sql, &[]).await?;
    }
    Ok(deleted)
}

/// One round of housekeeping; failures are logged.
pub async fn run_once(state: &SharedState) {
    match crate::sync::remove_orphan_blobs(state, crate::sync::ORPHAN_MIN_AGE).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(files = n, "removed unused telemetry files"),
        Err(e) => tracing::warn!(error = %e, "cleaning up telemetry files failed"),
    }
    let rows = async { remove_expired(&state.conn().await?).await }.await;
    match rows {
        Ok(0) => {}
        Ok(n) => tracing::info!(rows = n, "removed expired sessions, codes and tokens"),
        Err(e) => tracing::warn!(error = %e, "removing expired rows failed"),
    }
}

/// Runs [`run_once`] every hour.
pub async fn run_periodically(state: SharedState) {
    let mut every = tokio::time::interval(EVERY);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        every.tick().await;
        run_once(&state).await;
    }
}
