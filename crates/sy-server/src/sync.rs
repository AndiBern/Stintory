// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sync of a user's own sessions and laps between their devices
//! (docs/ARCHITECTURE.md §9.5).
//!
//! # Push (client → server)
//!
//! 1. `POST /sync/blobs/missing {hashes}`: which telemetry files the server
//!    lacks;
//! 2. `PUT /sync/blobs/{hash}` for each of them (checked against the hash and
//!    decoded);
//! 3. `POST /sync/push {sessions, laps, deletedSessions}` in one
//!    transaction: sessions and laps are created or updated, deleted
//!    sessions removed.
//!
//! # Pull (server → client)
//!
//! `GET /sync/changes?since=N` returns everything changed after `N` in
//! change order, with a new cursor. Every write gets the next number of
//! `change_seq`; writes of one user hold an exclusive advisory lock and
//! reads a shared one, so a client never skips a change that commits later
//! with a lower number.

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use deadpool_postgres::GenericClient;
use http_body_util::BodyExt;
use serde::Deserialize;
use sy_model::remote::{BlobHashes, MissingBlobs, SyncChanges, SyncPush, SyncPushResult};
use sy_model::{BlobHash, Lap, LapTelemetry, Session, SessionId};
use uuid::Uuid;

use crate::auth::Auth;
use crate::data::{LAP_COLUMNS, lap_from_row};
use crate::error::{ApiError, ApiResult};
use crate::state::SharedState;

/// Serializes the writes of one user (exclusive) against reads of their
/// change feed (shared). Held until the transaction ends.
pub async fn lock_user(db: &impl GenericClient, user: Uuid, exclusive: bool) -> ApiResult<()> {
    let sql = if exclusive {
        "SELECT pg_advisory_xact_lock(hashtextextended($1::text, 7428))"
    } else {
        "SELECT pg_advisory_xact_lock_shared(hashtextextended($1::text, 7428))"
    };
    db.execute(sql, &[&user.to_string()]).await?;
    Ok(())
}

/// Deletes sessions of `owner` (with their laps) and records tombstones for
/// other devices. Returns blob hashes that may have become unused.
pub async fn delete_sessions(
    db: &impl GenericClient,
    owner: Uuid,
    ids: &[Uuid],
) -> ApiResult<(u64, Vec<String>)> {
    if ids.is_empty() {
        return Ok((0, Vec::new()));
    }
    let hashes: Vec<String> = db
        .query(
            "SELECT DISTINCT blob_hash FROM laps WHERE owner_id = $1 AND session_id = ANY($2)",
            &[&owner, &ids],
        )
        .await?
        .iter()
        .map(|r| r.get(0))
        .collect();
    db.execute(
        "DELETE FROM share_grants
         WHERE owner_id = $1
           AND ((object_kind = 'session' AND object_id = ANY($2))
             OR (object_kind = 'lap' AND object_id IN (
                    SELECT id FROM laps WHERE owner_id = $1 AND session_id = ANY($2))))",
        &[&owner, &ids],
    )
    .await?;
    let deleted = db
        .execute(
            "DELETE FROM sessions WHERE owner_id = $1 AND id = ANY($2)",
            &[&owner, &ids],
        )
        .await?;
    db.execute(
        "INSERT INTO deletions (owner_id, kind, id)
         SELECT $1, 'session', unnest($2::uuid[])
         ON CONFLICT (owner_id, kind, id) DO UPDATE SET seq = nextval('change_seq')",
        &[&owner, &ids],
    )
    .await?;
    Ok((deleted, hashes))
}

/// Removes telemetry files no lap references anymore (after commit). While
/// files are being stored, it leaves them to the periodic cleanup
/// ([`remove_orphan_blobs`]) instead of waiting.
pub async fn remove_unused_blobs(state: &SharedState, hashes: Vec<String>) -> ApiResult<()> {
    if hashes.is_empty() {
        return Ok(());
    }
    let Ok(_exclusive) = state.blob_writes.try_write() else {
        return Ok(());
    };
    let orphans: Vec<String> = state
        .conn()
        .await?
        .query(
            "DELETE FROM blobs b WHERE b.hash = ANY($1)
               AND NOT EXISTS (SELECT 1 FROM laps l WHERE l.blob_hash = b.hash)
             RETURNING b.hash",
            &[&hashes],
        )
        .await?
        .iter()
        .map(|r| r.get(0))
        .collect();
    for hash in orphans {
        if let Ok(hash) = hash.parse::<BlobHash>() {
            state.blobs.remove(&hash)?;
        }
    }
    Ok(())
}

/// Unused telemetry files are removed once they are this old (a sync
/// uploads files before the laps that refer to them; this leaves it time).
pub const ORPHAN_MIN_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Removes telemetry files no lap refers to and that are older than
/// `min_age`: uploads whose laps never came, files of failed imports, and
/// files left over after deletions. Returns how many files were removed.
/// Skips the run (0) while files are being stored.
pub async fn remove_orphan_blobs(
    state: &SharedState,
    min_age: std::time::Duration,
) -> ApiResult<usize> {
    let Ok(_exclusive) = state.blob_writes.try_write() else {
        return Ok(0);
    };
    let db = state.conn().await?;
    // known files without laps
    let mut remove: std::collections::HashSet<String> = db
        .query(
            "DELETE FROM blobs b WHERE b.created_at < now() - make_interval(secs => $1)
               AND NOT EXISTS (SELECT 1 FROM laps l WHERE l.blob_hash = b.hash)
             RETURNING b.hash",
            &[&min_age.as_secs_f64()],
        )
        .await?
        .iter()
        .map(|r| r.get(0))
        .collect();
    // files the database doesn't know (anymore)
    let blobs = state.blobs.clone();
    let on_disk = tokio::task::spawn_blocking(move || blobs.list()).await??;
    let cutoff = std::time::SystemTime::now() - min_age;
    let old: Vec<String> = on_disk
        .into_iter()
        .filter(|(hash, modified)| *modified < cutoff && !remove.contains(hash.as_str()))
        .map(|(hash, _)| hash.as_str().to_owned())
        .collect();
    for chunk in old.chunks(1000) {
        let known: std::collections::HashSet<String> = db
            .query("SELECT hash FROM blobs WHERE hash = ANY($1)", &[&chunk])
            .await?
            .iter()
            .map(|r| r.get(0))
            .collect();
        remove.extend(chunk.iter().filter(|h| !known.contains(*h)).cloned());
    }
    let count = remove.len();
    let blobs = state.blobs.clone();
    tokio::task::spawn_blocking(move || {
        remove
            .iter()
            .filter_map(|h| h.parse::<BlobHash>().ok())
            .try_for_each(|h| blobs.remove(&h))
    })
    .await??;
    Ok(count)
}

// ----- push ---------------------------------------------------------------

/// Which of the given telemetry files the user has to upload: those they
/// haven't uploaded (or imported) themselves, even if another user did. So
/// the answer doesn't tell whether the server has a file, and a lap can
/// only refer to a file its owner has.
pub async fn missing_blobs(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Json(req): Json<BlobHashes>,
) -> ApiResult<Json<MissingBlobs>> {
    if req.hashes.len() > 10_000 {
        return Err(ApiError::bad_request(
            "too many hashes, send them in batches",
        ));
    }
    let known: Vec<String> = state
        .conn()
        .await?
        .query(
            "SELECT hash FROM blob_uploads WHERE user_id = $1 AND hash = ANY($2)",
            &[&session.user.id, &req.hashes],
        )
        .await?
        .iter()
        .map(|r| r.get(0))
        .collect();
    Ok(Json(MissingBlobs {
        missing: req
            .hashes
            .into_iter()
            .filter(|h| !known.contains(h))
            .collect(),
    }))
}

/// Writes a request body of at most `limit` bytes to a temporary file;
/// returns the file and the body's SHA-256 (hex). The file is deleted when
/// dropped.
///
/// The limit is checked here because axum's `DefaultBodyLimit` doesn't
/// apply to a raw [`Body`] extractor.
pub(crate) async fn spool_body(
    state: &SharedState,
    body: Body,
    limit: usize,
) -> ApiResult<(tempfile::NamedTempFile, String)> {
    use sha2::Digest;
    use std::io::Write;
    let tmp_dir = state.config.data_dir.join("tmp");
    std::fs::create_dir_all(&tmp_dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(&tmp_dir)?;
    let mut sha256 = sha2::Sha256::new();
    let mut body = http_body_util::Limited::new(body, limit);
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| {
            if e.is::<http_body_util::LengthLimitError>() {
                ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    format!(
                        "the upload is larger than {} MiB",
                        limit.div_ceil(1024 * 1024)
                    ),
                )
            } else {
                ApiError::bad_request(format!("upload failed: {e}"))
            }
        })?;
        if let Some(data) = frame.data_ref() {
            sha256.update(data);
            tmp.write_all(data)?;
        }
    }
    tmp.flush()?;
    Ok((tmp, hex::encode(sha256.finalize())))
}

/// Waits for one of the [`crate::state::DECODE_SLOTS`]; hold it while a
/// telemetry file is in memory.
pub(crate) async fn decode_slot(
    state: &SharedState,
) -> ApiResult<tokio::sync::SemaphorePermit<'_>> {
    state
        .decode_slots
        .acquire()
        .await
        .map_err(|_| ApiError::internal("decode slots closed"))
}

/// `PUT /sync/blobs/{hash}`: uploads one telemetry file (Parquet).
pub async fn put_blob(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Path(hash): Path<String>,
    body: Body,
) -> ApiResult<StatusCode> {
    let expected: BlobHash = hash
        .parse()
        .map_err(|_| ApiError::bad_request("invalid hash"))?;
    // to disk first: slow or many parallel uploads don't hold memory
    let (tmp, sha256) = spool_body(&state, body, state.config.blob_limit()).await?;
    if sha256 != expected.as_str() {
        return Err(ApiError::bad_request("content does not match the hash"));
    }
    let _slot = decode_slot(&state).await?;
    let _writes = state.blob_writes.read().await;
    let state2 = state.clone();
    let hash = expected.clone();
    let size = tokio::task::spawn_blocking(move || -> ApiResult<u64> {
        // a stored file was checked when it was stored (same hash, same bytes)
        if !state2.blobs.contains(&hash) {
            let bytes = axum::body::Bytes::from(std::fs::read(tmp.path())?);
            LapTelemetry::from_parquet(bytes.clone())
                .map_err(|e| ApiError::bad_request(format!("not a telemetry file: {e}")))?;
            state2.blobs.put(&bytes)?;
        }
        Ok(tmp.as_file().metadata()?.len())
    })
    .await??;
    record_upload(&state.conn().await?, session.user.id, &expected, size).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Records a stored telemetry file and that `user` has it (uploaded or
/// imported), so their laps may refer to it.
pub(crate) async fn record_upload(
    db: &impl GenericClient,
    user: Uuid,
    hash: &BlobHash,
    size: u64,
) -> ApiResult<()> {
    db.execute(
        "INSERT INTO blobs (hash, size) VALUES ($1, $2) ON CONFLICT (hash) DO NOTHING",
        &[&hash.as_str(), &(size as i64)],
    )
    .await?;
    db.execute(
        "INSERT INTO blob_uploads (user_id, hash) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        &[&user, &hash.as_str()],
    )
    .await?;
    Ok(())
}

/// Most objects per push request.
pub const MAX_PUSH: usize = 2_000;

/// Writes sessions and laps of the device's user.
pub async fn push(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Json(req): Json<SyncPush>,
) -> ApiResult<Json<SyncPushResult>> {
    if req.sessions.len() + req.laps.len() + req.deleted_sessions.len() > MAX_PUSH {
        return Err(ApiError::bad_request(
            "too many objects, send them in batches",
        ));
    }
    let owner = session.user.id;
    // laps refer to uploaded files: keep the cleanup from removing them now
    let writes = state.blob_writes.read().await;
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    lock_user(&tx, owner, true).await?;
    let mut result = SyncPushResult::default();

    let deleted: Vec<Uuid> = req
        .deleted_sessions
        .iter()
        .map(|id| *id.as_uuid())
        .collect();
    let (n, hashes) = delete_sessions(&tx, owner, &deleted).await?;
    result.deleted_sessions = n as u32;

    for s in &req.sessions {
        upsert_session(&tx, owner, s).await?;
        result.sessions += 1;
    }
    for lap in &req.laps {
        if upsert_lap(&tx, owner, lap).await? {
            result.laps += 1;
        }
    }
    tx.commit().await?;
    drop(writes);
    remove_unused_blobs(&state, hashes).await?;
    Ok(Json(result))
}

async fn upsert_session(db: &impl GenericClient, owner: Uuid, s: &Session) -> ApiResult<()> {
    db.execute(
        "INSERT INTO sessions (owner_id, id, game, game_version, track_key, track_name, car_key,
            car_name, session_type, started_at, ended_at, conditions, setup, session_info, origin)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
         ON CONFLICT (owner_id, id) DO UPDATE SET
            game_version = EXCLUDED.game_version, track_key = EXCLUDED.track_key,
            track_name = EXCLUDED.track_name, car_key = EXCLUDED.car_key,
            car_name = EXCLUDED.car_name, session_type = EXCLUDED.session_type,
            ended_at = EXCLUDED.ended_at, conditions = EXCLUDED.conditions,
            setup = EXCLUDED.setup, session_info = EXCLUDED.session_info,
            updated_at = now(), seq = nextval('change_seq')",
        &[
            &owner,
            s.id.as_uuid(),
            &s.game.as_str(),
            &s.game_version,
            &s.track_key,
            &s.track_name,
            &s.car_key,
            &s.car_name,
            &s.session_type.as_str(),
            &s.started_at,
            &s.ended_at,
            &serde_json::to_value(&s.conditions)?,
            &s.setup,
            &s.session_info,
            &serde_json::to_value(&s.origin)?,
        ],
    )
    .await?;
    // a session that comes back (e.g. pushed by a device that missed the
    // deletion) is not deleted anymore
    db.execute(
        "DELETE FROM deletions WHERE owner_id = $1 AND kind = 'session' AND id = $2",
        &[&owner, s.id.as_uuid()],
    )
    .await?;
    Ok(())
}

async fn upsert_lap(db: &impl GenericClient, owner: Uuid, lap: &Lap) -> ApiResult<bool> {
    // the owner must have uploaded the file: knowing its hash isn't enough
    let known_blob = db
        .query_opt(
            "SELECT 1 FROM blob_uploads WHERE user_id = $1 AND hash = $2",
            &[&owner, &lap.blob_hash.as_str()],
        )
        .await?
        .is_some();
    if !known_blob {
        return Err(ApiError::bad_request(format!(
            "telemetry {} of lap {} is not uploaded",
            lap.blob_hash, lap.id
        )));
    }
    let known_session = db
        .query_opt(
            "SELECT 1 FROM sessions WHERE owner_id = $1 AND id = $2",
            &[&owner, lap.session_id.as_uuid()],
        )
        .await?
        .is_some();
    if !known_session {
        return Err(ApiError::bad_request(format!(
            "session {} of lap {} is unknown",
            lap.session_id, lap.id
        )));
    }
    let n = db
        .execute(
            "INSERT INTO laps (owner_id, id, session_id, number, lap_time_ms, sector_times_ms,
                valid, invalid_reason, flags, blob_hash, sample_rate_hz, sample_count, start_time_s)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
             ON CONFLICT (owner_id, id) DO UPDATE SET
                number = EXCLUDED.number, lap_time_ms = EXCLUDED.lap_time_ms,
                sector_times_ms = EXCLUDED.sector_times_ms, valid = EXCLUDED.valid,
                invalid_reason = EXCLUDED.invalid_reason, flags = EXCLUDED.flags,
                blob_hash = EXCLUDED.blob_hash, sample_rate_hz = EXCLUDED.sample_rate_hz,
                sample_count = EXCLUDED.sample_count, start_time_s = EXCLUDED.start_time_s,
                updated_at = now(), seq = nextval('change_seq')",
            &[
                &owner,
                lap.id.as_uuid(),
                lap.session_id.as_uuid(),
                &(lap.number as i32),
                &lap.lap_time_ms.map(|v| v as i32),
                &serde_json::to_value(&lap.sector_times_ms)?,
                &lap.valid,
                &lap.invalid_reason,
                &serde_json::to_value(lap.flags)?,
                &lap.blob_hash.as_str(),
                &lap.sample_rate_hz,
                &(lap.sample_count as i32),
                &lap.start_time_s,
            ],
        )
        .await?;
    Ok(n > 0)
}

// ----- pull ---------------------------------------------------------------

/// `GET /sync/changes?since=&limit=`
#[derive(Debug, Deserialize)]
pub struct ChangesQuery {
    /// Cursor of the last pull (0 = everything)
    #[serde(default)]
    pub since: i64,
    /// Most changes per answer (default 500)
    pub limit: Option<i64>,
}

/// What changed for the user since a cursor.
pub async fn changes(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Query(q): Query<ChangesQuery>,
) -> ApiResult<Json<SyncChanges>> {
    let owner = session.user.id;
    let limit = q.limit.unwrap_or(500).clamp(1, 2000);
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    lock_user(&tx, owner, false).await?;
    let feed = tx
        .query(
            "SELECT seq, kind, id FROM (
                SELECT seq, 'session' AS kind, id FROM sessions WHERE owner_id = $1 AND seq > $2
                UNION ALL
                SELECT seq, 'lap', id FROM laps WHERE owner_id = $1 AND seq > $2
                UNION ALL
                SELECT seq, 'deleted', id FROM deletions WHERE owner_id = $1 AND seq > $2
             ) changes ORDER BY seq LIMIT $3",
            &[&owner, &q.since, &limit],
        )
        .await?;
    let mut out = SyncChanges {
        cursor: q.since,
        more: feed.len() as i64 == limit,
        ..Default::default()
    };
    let (mut session_ids, mut lap_ids) = (Vec::<Uuid>::new(), Vec::<Uuid>::new());
    for row in &feed {
        out.cursor = row.get(0);
        let id: Uuid = row.get(2);
        match row.get::<_, &str>(1) {
            "session" => session_ids.push(id),
            "lap" => lap_ids.push(id),
            _ => out.deleted_sessions.push(SessionId::from_uuid(id)),
        }
    }
    if !lap_ids.is_empty() {
        let rows = tx
            .query(
                &format!(
                    "SELECT {LAP_COLUMNS} FROM laps WHERE owner_id = $1 AND id = ANY($2) ORDER BY seq"
                ),
                &[&owner, &lap_ids],
            )
            .await?;
        for r in &rows {
            let lap = lap_from_row(r)?;
            // the receiver needs the lap's session even if it changed earlier
            if !session_ids.contains(lap.session_id.as_uuid()) {
                session_ids.push(*lap.session_id.as_uuid());
            }
            out.laps.push(lap);
        }
    }
    if !session_ids.is_empty() {
        let rows = tx
            .query(
                &format!(
                    "SELECT {SESSION_COLUMNS} FROM sessions WHERE owner_id = $1 AND id = ANY($2)"
                ),
                &[&owner, &session_ids],
            )
            .await?;
        out.sessions = rows
            .iter()
            .map(session_from_row)
            .collect::<ApiResult<_>>()?;
        out.sessions.sort_by_key(|s| s.started_at);
    }
    tx.commit().await?;
    Ok(Json(out))
}

const SESSION_COLUMNS: &str = "id, game, game_version, track_key, track_name, car_key, car_name, \
     session_type, started_at, ended_at, conditions, setup, session_info, origin";

fn session_from_row(r: &tokio_postgres::Row) -> ApiResult<Session> {
    let parse = |v: &str, what: &str| ApiError::internal(format!("bad {what} in database: {v:?}"));
    let game: &str = r.get(1);
    let session_type: &str = r.get(7);
    Ok(Session {
        id: SessionId::from_uuid(r.get(0)),
        game: game.parse().map_err(|_| parse(game, "game"))?,
        game_version: r.get(2),
        track_key: r.get(3),
        track_name: r.get(4),
        car_key: r.get(5),
        car_name: r.get(6),
        session_type: session_type
            .parse()
            .map_err(|_| parse(session_type, "session type"))?,
        started_at: r.get(8),
        ended_at: r.get(9),
        conditions: serde_json::from_value(r.get(10))?,
        setup: r.get(11),
        session_info: r.get(12),
        origin: serde_json::from_value(r.get(13))?,
    })
}

/// `GET /sync/blobs/{hash}`: a telemetry file of one of the user's laps, or
/// of a lap shared with them.
pub async fn get_blob(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Path(hash): Path<String>,
) -> ApiResult<Response> {
    let hash: BlobHash = hash.parse().map_err(|_| ApiError::not_found("telemetry"))?;
    let readable =
        crate::sharing::blob_readable(&state.conn().await?, session.user.id, hash.as_str()).await?;
    if !readable {
        return Err(ApiError::not_found("telemetry"));
    }
    let state2 = state.clone();
    let bytes = tokio::task::spawn_blocking(move || state2.blobs.get(&hash)).await??;
    Ok((
        [(header::CONTENT_TYPE, "application/vnd.apache.parquet")],
        bytes,
    )
        .into_response())
}
