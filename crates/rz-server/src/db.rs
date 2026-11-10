// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Postgres connection pool and schema migrations.
//!
//! Migrations are the SQL files in `migrations/`, embedded at compile time
//! and applied in order. Applied versions are recorded in
//! `schema_migrations`; a server refuses to start on a newer schema.

use std::time::{Duration, Instant};

use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod};
use tokio_postgres::NoTls;
use tokio_postgres::config::Host;

use crate::error::ApiError;

/// Embedded migrations: (version, SQL).
const MIGRATIONS: &[(i32, &str)] = &[
    (1, include_str!("../migrations/0001_initial.sql")),
    (2, include_str!("../migrations/0002_devices_and_sync.sql")),
];

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

/// `user@host:port/database` of a connection URL, without the password
/// (for log messages).
pub fn describe(url: &str) -> String {
    let Ok(c) = url.parse::<tokio_postgres::Config>() else {
        return "(invalid database URL)".into();
    };
    let host = c
        .get_hosts()
        .first()
        .map(|h| match h {
            Host::Tcp(name) => name.clone(),
            #[cfg(unix)]
            Host::Unix(path) => path.display().to_string(),
        })
        .unwrap_or_else(|| "localhost".into());
    let port = c.get_ports().first().copied().unwrap_or(5432);
    format!(
        "{}@{host}:{port}/{}",
        c.get_user().unwrap_or("?"),
        c.get_dbname().or(c.get_user()).unwrap_or("?")
    )
}

/// Waits until the database accepts a connection, retrying with growing
/// pauses (1 s … 30 s) for at most `max`. Every failed attempt is logged
/// with the reason, e.g. a wrong password or an unknown host.
pub async fn wait_until_reachable(
    pool: &Pool,
    target: &str,
    max: Duration,
) -> Result<(), ApiError> {
    let start = Instant::now();
    let mut pause = Duration::from_secs(1);
    loop {
        match pool.get().await {
            Ok(_) => return Ok(()),
            Err(e) => {
                let e = crate::error::chain(&e);
                let waited = start.elapsed();
                if waited + pause > max {
                    return Err(ApiError::internal(format!(
                        "database at {target} not reachable (gave up after {} s): {e}",
                        waited.as_secs()
                    )));
                }
                tracing::warn!(
                    database = target,
                    error = e,
                    "waiting for the database, retrying in {} s",
                    pause.as_secs()
                );
                tokio::time::sleep(pause).await;
                pause = (pause * 2).min(Duration::from_secs(30));
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_hides_the_password() {
        let d = describe("postgres://rz:secret@db.internal:5433/laps");
        assert_eq!(d, "rz@db.internal:5433/laps");
        assert_eq!(describe("postgres://rz:secret@db"), "rz@db:5432/rz");
        assert!(!describe("nonsense").contains("nonsense"));
    }

    #[tokio::test]
    async fn gives_up_with_a_clear_message() {
        // nothing listens on port 1
        let url = "postgres://rz:secret@127.0.0.1:1/laps";
        let pool = pool(url, None).unwrap();
        let err = wait_until_reachable(&pool, &describe(url), Duration::ZERO)
            .await
            .unwrap_err();
        let msg = err.message();
        assert!(msg.contains("rz@127.0.0.1:1/laps"), "{msg}");
        assert!(!msg.contains("secret"), "{msg}");
    }
}
