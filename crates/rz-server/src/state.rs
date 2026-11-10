// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared server state, server settings and the audit log.

use std::sync::Arc;

use deadpool_postgres::{GenericClient, Pool};
use rz_store::BlobStore;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::Config;
use crate::crypto::SecretBox;
use crate::error::{ApiError, ApiResult};
use crate::ratelimit::RateLimiter;

/// State shared by all requests.
#[derive(Debug)]
pub struct AppState {
    /// Configuration
    pub config: Config,
    /// Database pool
    pub db: Pool,
    /// Telemetry files
    pub blobs: BlobStore,
    /// Server key
    pub secrets: SecretBox,
    /// Failed login counters
    pub limiter: RateLimiter,
}

/// Handle to the shared state.
pub type SharedState = Arc<AppState>;

/// Who may create an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Registration {
    /// Only admins create accounts
    #[default]
    Closed,
    /// Anyone who can reach the server
    Open,
}

/// Settings an admin can change at runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettings {
    /// Who may create an account
    pub registration: Registration,
    /// Every user must set up a second factor
    pub enforce_2fa: bool,
    /// Sharing with "everyone on this server" is possible (M3b)
    pub allow_share_everyone: bool,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            registration: Registration::Closed,
            enforce_2fa: false,
            allow_share_everyone: true,
        }
    }
}

const SETTINGS_KEY: &str = "settings";

impl ServerSettings {
    /// Current settings (defaults if never saved).
    pub async fn load(db: &impl GenericClient) -> ApiResult<Self> {
        let row = db
            .query_opt(
                "SELECT value FROM server_settings WHERE key = $1",
                &[&SETTINGS_KEY],
            )
            .await?;
        Ok(match row {
            // missing fields (added in later versions) take their defaults
            Some(r) => {
                let mut value: serde_json::Value = r.get(0);
                let defaults = serde_json::to_value(Self::default())?;
                if let (Some(v), serde_json::Value::Object(d)) = (value.as_object_mut(), defaults) {
                    for (k, dv) in d {
                        v.entry(k).or_insert(dv);
                    }
                }
                serde_json::from_value(value)?
            }
            None => Self::default(),
        })
    }

    /// Saves the settings.
    pub async fn save(&self, db: &impl GenericClient) -> ApiResult<()> {
        db.execute(
            "INSERT INTO server_settings (key, value) VALUES ($1, $2)
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
            &[&SETTINGS_KEY, &serde_json::to_value(self)?],
        )
        .await?;
        Ok(())
    }
}

/// Writes a security event to the audit log (docs/PLAN.md §9.3). Failures
/// are logged, not returned: auditing must not break the action itself.
pub async fn audit(
    db: &impl GenericClient,
    user: Option<Uuid>,
    event: &str,
    ip: Option<&str>,
    details: serde_json::Value,
) {
    let result = db
        .execute(
            "INSERT INTO audit_log (user_id, event, ip, details) VALUES ($1, $2, $3, $4)",
            &[&user, &event, &ip, &details],
        )
        .await;
    if let Err(e) = result {
        tracing::error!(error = %e, event, "audit log write failed");
    }
    tracing::info!(event, user = ?user, ip, "audit");
}

impl AppState {
    /// A database connection.
    pub async fn conn(&self) -> ApiResult<deadpool_postgres::Object> {
        Ok(self.db.get().await?)
    }
}

impl From<rz_store::StoreError> for ApiError {
    fn from(e: rz_store::StoreError) -> Self {
        ApiError::internal(format!("blob store: {e}"))
    }
}
