// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Postgres connection pool and schema migrations.
//!
//! Migrations are the SQL files in `migrations/`, embedded at compile time
//! and applied in order. Applied versions are recorded in
//! `schema_migrations`; a server refuses to start on a newer schema.

use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod};
use tokio_postgres::NoTls;

use crate::error::ApiError;

/// Embedded migrations: (version, SQL).
const MIGRATIONS: &[(i32, &str)] = &[(1, include_str!("../migrations/0001_initial.sql"))];

/// Creates a connection pool. `search_path` selects a schema (tests).
pub fn pool(url: &str, search_path: Option<&str>) -> Result<Pool, ApiError> {
    let mut config: tokio_postgres::Config = url
        .parse()
        .map_err(|e| ApiError::internal(format!("invalid database URL: {e}")))?;
    if let Some(schema) = search_path {
        config.options(format!("-c search_path={schema}"));
    }
    config.application_name("rz-server");
    let manager = Manager::from_config(
        config,
        NoTls,
        ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        },
    );
    Pool::builder(manager)
        .max_size(16)
        .build()
        .map_err(|e| ApiError::internal(format!("database pool: {e}")))
}

/// Applies all migrations not applied yet. Runs under an advisory lock, so
/// several server instances starting together don't race.
pub async fn migrate(pool: &Pool) -> Result<(), ApiError> {
    let mut client = pool.get().await?;
    let tx = client.transaction().await?;
    tx.execute("SELECT pg_advisory_xact_lock(7428301)", &[])
        .await?;
    tx.batch_execute(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version     integer PRIMARY KEY,
            applied_at  timestamptz NOT NULL DEFAULT now()
        )",
    )
    .await?;
    let current: Option<i32> = tx
        .query_one("SELECT max(version) FROM schema_migrations", &[])
        .await?
        .get(0);
    let current = current.unwrap_or(0);
    let latest = MIGRATIONS.last().map_or(0, |m| m.0);
    if current > latest {
        return Err(ApiError::internal(format!(
            "database schema version {current} is newer than this server ({latest}); please update rz-server"
        )));
    }
    for (version, sql) in MIGRATIONS.iter().filter(|m| m.0 > current) {
        tx.batch_execute(sql).await?;
        tx.execute(
            "INSERT INTO schema_migrations (version) VALUES ($1)",
            &[version],
        )
        .await?;
        tracing::info!(version, "database migrated");
    }
    tx.commit().await?;
    Ok(())
}
