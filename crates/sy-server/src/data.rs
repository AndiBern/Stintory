// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sessions and laps of the logged-in user: the same views the desktop app
//! has (sessions list, session detail, lap comparison), plus `.stintory` upload.
//!
//! Telemetry files are content addressed on disk ([`sy_store::BlobStore`]);
//! the database has the metadata.

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use deadpool_postgres::GenericClient;
use sy_analysis::{DEFAULT_STEP_M, MIN_STEP_M};
use sy_model::api::{
    AppInfo, BackendKind, CompareData, CompareRequest, ConditionsView, ImportResult, LapSummary,
    SessionDetail, SessionSummary,
};
use sy_model::bundle::BundleReader;
use sy_model::{GameId, Lap, LapId, LapTelemetry, Origin, Session, SessionId};
use tokio_postgres::Row;
use uuid::Uuid;

use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::state::SharedState;

/// `GET /info`
pub async fn info() -> Json<AppInfo> {
    Json(AppInfo {
        name: "Stintory".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        backend: BackendKind::Server,
    })
}

fn parse<T: std::str::FromStr>(value: &str, column: &str) -> ApiResult<T> {
    value
        .parse()
        .map_err(|_| ApiError::internal(format!("bad value in {column}: {value:?}")))
}

/// Laps that count for "best lap": valid, complete, not out-/in-lap.
pub(crate) const CLEAN_LAP: &str = "l.valid
    AND NOT coalesce((l.flags->>'out_lap')::boolean, false)
    AND NOT coalesce((l.flags->>'in_lap')::boolean, false)
    AND NOT coalesce((l.flags->>'incomplete')::boolean, false)";

fn summary_from_row(r: &Row) -> ApiResult<SessionSummary> {
    let origin: Origin = serde_json::from_value(r.get(8))?;
    Ok(SessionSummary {
        id: SessionId::from_uuid(r.get(0)),
        game: parse(r.get(1), "sessions.game")?,
        track_key: r.get(2),
        car_key: r.get(3),
        session_type: parse(r.get(4), "sessions.session_type")?,
        started_at: r.get(5),
        track_name: r.get(6),
        car_name: r.get(7),
        lap_count: u32::try_from(r.get::<_, i64>(9)).unwrap_or(0),
        best_lap_ms: r.get::<_, Option<i32>>(10).map(|v| v as u32),
        origin: origin.into(),
        shared: r.get(11),
    })
}

async fn summaries(
    db: &impl GenericClient,
    owner: Uuid,
    session: Option<Uuid>,
) -> ApiResult<Vec<SessionSummary>> {
    let everyone = crate::state::ServerSettings::load(db)
        .await?
        .allow_share_everyone;
    let rows = db
        .query(
            &format!(
                "SELECT s.id, s.game, s.track_key, s.car_key, s.session_type, s.started_at,
                        s.track_name, s.car_name, s.origin, count(l.id),
                        min(CASE WHEN {CLEAN_LAP} THEN l.lap_time_ms END),
                        EXISTS (
                            SELECT 1 FROM share_grants g
                            WHERE g.owner_id = s.owner_id
                              AND (g.grantee_kind <> 'everyone' OR $3)
                              AND ((s.sharing_mode = 'custom' AND g.object_kind = 'session'
                                    AND g.object_id = s.id)
                                OR (s.sharing_mode = 'inherit' AND s.origin->>'kind' = 'own'
                                    AND g.object_kind = 'user_default'
                                    AND g.object_id = s.owner_id)))
                 FROM sessions s
                 LEFT JOIN laps l ON l.owner_id = s.owner_id AND l.session_id = s.id
                 WHERE s.owner_id = $1 AND ($2::uuid IS NULL OR s.id = $2)
                 GROUP BY s.owner_id, s.id
                 ORDER BY s.started_at DESC"
            ),
            &[&owner, &session, &everyone],
        )
        .await?;
    rows.iter().map(summary_from_row).collect()
}

/// `GET /sessions`: the user's sessions, newest first.
pub async fn list_sessions(
    State(state): State<SharedState>,
    Auth(session): Auth,
) -> ApiResult<Json<Vec<SessionSummary>>> {
    Ok(Json(
        summaries(&state.conn().await?, session.user.id, None).await?,
    ))
}

/// The columns [`lap_from_row`] reads. Laps of other users go through
/// `sharing::viewer_lap_columns`, which leaves out a setup they may not see.
pub(crate) const LAP_COLUMNS: &str = "id, session_id, number, lap_time_ms, sector_times_ms, valid, \
     invalid_reason, flags, blob_hash, sample_rate_hz, sample_count, start_time_s, setup";

pub(crate) fn lap_from_row(r: &Row) -> ApiResult<Lap> {
    Ok(Lap {
        id: LapId::from_uuid(r.get(0)),
        session_id: SessionId::from_uuid(r.get(1)),
        number: u32::try_from(r.get::<_, i32>(2)).unwrap_or(0),
        lap_time_ms: r.get::<_, Option<i32>>(3).map(|v| v as u32),
        sector_times_ms: serde_json::from_value(r.get(4))?,
        valid: r.get(5),
        invalid_reason: r.get(6),
        flags: serde_json::from_value(r.get(7))?,
        blob_hash: parse(r.get(8), "laps.blob_hash")?,
        sample_rate_hz: r.get(9),
        sample_count: u32::try_from(r.get::<_, i32>(10)).unwrap_or(0),
        start_time_s: r.get(11),
        setup: r.get(12),
    })
}

/// `GET /sessions/{id}`
pub async fn get_session(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<SessionDetail>> {
    let db = state.conn().await?;
    let owner = session.user.id;
    let summary = summaries(&db, owner, Some(id))
        .await?
        .pop()
        .ok_or_else(|| ApiError::not_found("session"))?;
    let row = db
        .query_one(
            "SELECT ended_at, game_version, conditions, session_info FROM sessions
             WHERE owner_id = $1 AND id = $2",
            &[&owner, &id],
        )
        .await?;
    let conditions: sy_model::session::Conditions = serde_json::from_value(row.get(2))?;
    let info: Option<serde_json::Value> = row.get(3);
    let laps = db
        .query(
            &format!(
                "SELECT {LAP_COLUMNS} FROM laps WHERE owner_id = $1 AND session_id = $2
                 ORDER BY number, id"
            ),
            &[&owner, &id],
        )
        .await?;
    let laps = laps
        .iter()
        .map(lap_from_row)
        .collect::<ApiResult<Vec<_>>>()?;
    let laps = lap_summaries(&state, summary.game, info.as_ref(), laps).await?;
    Ok(Json(SessionDetail {
        summary,
        ended_at: row.get(0),
        game_version: row.get(1),
        conditions: ConditionsView::from(&conditions),
        laps,
    }))
}

/// The lap table rows, with weather, fuel and tyres from each lap's
/// telemetry (cached, see `sy_analysis::laps_conditions`). A lap whose telemetry
/// can't be read just has no conditions.
pub(crate) async fn lap_summaries(
    state: &SharedState,
    game: GameId,
    session_info: Option<&serde_json::Value>,
    laps: Vec<Lap>,
) -> ApiResult<Vec<LapSummary>> {
    let state = state.clone();
    // the tyre compound names (the session info itself stays with the owner)
    let names = sy_analysis::CompoundNames::from_session_info(game, session_info);
    Ok(tokio::task::spawn_blocking(move || {
        let hashes: Vec<_> = laps.iter().map(|l| &l.blob_hash).collect();
        let conditions =
            sy_analysis::laps_conditions(game, &names, &hashes, |h| state.blobs.get(h));
        laps.iter()
            .zip(conditions)
            .map(|(lap, conditions)| LapSummary {
                conditions: conditions
                    .inspect_err(|e| tracing::warn!(lap = %lap.id, error = %e, "lap conditions"))
                    .ok(),
                ..LapSummary::from(lap)
            })
            .collect()
    })
    .await?)
}

/// `DELETE /sessions/{id}`: deletes the session with its laps; telemetry
/// files nobody references anymore are removed.
pub async fn delete_session(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    crate::sync::lock_user(&tx, session.user.id, true).await?;
    let (n, hashes) = crate::sync::delete_sessions(&tx, session.user.id, &[id]).await?;
    if n == 0 {
        return Err(ApiError::not_found("session"));
    }
    tx.commit().await?;
    crate::sync::remove_unused_blobs(&state, hashes).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /analysis/compare`: like the desktop app, for the user's own laps
/// and laps shared with them.
pub async fn compare(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Json(request): Json<CompareRequest>,
) -> ApiResult<Json<CompareData>> {
    if request.lap_ids.is_empty() || request.lap_ids.len() > 8 {
        return Err(ApiError::bad_request("select 1 to 8 laps"));
    }
    let step = request.step_m.unwrap_or(DEFAULT_STEP_M).max(MIN_STEP_M);
    let db = state.conn().await?;
    let mut laps = Vec::new();
    for id in &request.lap_ids {
        let lap = crate::sharing::readable_lap(&db, session.user.id, *id.as_uuid())
            .await?
            .ok_or_else(|| ApiError::not_found("lap"))?;
        laps.push(lap);
    }
    let state2 = state.clone();
    let data = tokio::task::spawn_blocking(move || -> ApiResult<CompareData> {
        let telemetry = laps
            .iter()
            .map(|r| {
                let bytes = state2.blobs.get(&r.lap.blob_hash)?;
                LapTelemetry::from_parquet(bytes)
                    .map_err(|e| ApiError::internal(format!("telemetry of lap {}: {e}", r.lap.id)))
            })
            .collect::<ApiResult<Vec<_>>>()?;
        let refs: Vec<_> = laps.iter().map(|r| &r.lap).zip(&telemetry).collect();
        let mut data =
            sy_analysis::compare(&refs, step).map_err(|e| ApiError::bad_request(e.to_string()))?;
        for (trace, r) in data.laps.iter_mut().zip(&laps) {
            trace.driver = r.driver();
            trace.owner = r.owner.clone();
            trace.setup = sy_analysis::setup::lap_setup(
                r.game,
                r.lap.setup.as_ref(),
                r.session_setup.as_ref(),
            );
            trace.setup_hidden = r.setup_hidden;
            trace.fixed_setup = r.fixed_setup;
        }
        Ok(data)
    })
    .await??;
    Ok(Json(data))
}

// ----- storing ----------------------------------------------------------

/// Stores a session for `owner` unless it exists. Returns true if new.
pub async fn insert_session(db: &impl GenericClient, owner: Uuid, s: &Session) -> ApiResult<bool> {
    let n = db
        .execute(
            "INSERT INTO sessions (owner_id, id, game, game_version, track_key, track_name, car_key,
                car_name, session_type, started_at, ended_at, conditions, setup, session_info, origin)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
             ON CONFLICT (owner_id, id) DO NOTHING",
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
    Ok(n > 0)
}

/// Stores a lap whose telemetry file is stored already. Returns true if new.
pub async fn insert_lap(
    db: &impl GenericClient,
    owner: Uuid,
    lap: &Lap,
    blob_size: u64,
) -> ApiResult<bool> {
    crate::sync::record_upload(db, owner, &lap.blob_hash, blob_size).await?;
    let n = db
        .execute(
            "INSERT INTO laps (owner_id, id, session_id, number, lap_time_ms, sector_times_ms,
                valid, invalid_reason, flags, blob_hash, sample_rate_hz, sample_count, start_time_s,
                setup)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
             ON CONFLICT (owner_id, id) DO NOTHING",
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
                &lap.setup,
            ],
        )
        .await?;
    Ok(n > 0)
}

/// What an `.stintory` file contains, telemetry already in the blob store.
struct Bundle {
    sessions: Vec<Session>,
    laps: Vec<(Lap, u64)>,
}

/// Reads an `.stintory` file and stores its telemetry files (blocking).
fn read_bundle(state: &SharedState, file: std::fs::File) -> ApiResult<Bundle> {
    let bad = |e: sy_model::bundle::BundleError| ApiError::bad_request(e.to_string());
    let mut reader = BundleReader::open(std::io::BufReader::new(file)).map_err(bad)?;
    let driver_name = reader.manifest().driver_name.clone();
    let mut sessions = reader.sessions().map_err(bad)?;
    for s in &mut sessions {
        s.origin = match std::mem::replace(&mut s.origin, Origin::Own) {
            Origin::Own => Origin::Imported {
                driver_name: driver_name.clone(),
            },
            Origin::Imported { driver_name } => Origin::Imported { driver_name },
            Origin::Synced { owner, .. } => Origin::Imported {
                driver_name: Some(owner),
            },
        };
    }
    let limit = state.config.import_unpacked_limit();
    let mut unpacked = 0u64;
    let mut laps = Vec::new();
    for mut lap in reader.laps().map_err(bad)? {
        let bytes = reader.telemetry(&lap).map_err(bad)?;
        unpacked += bytes.len() as u64;
        if unpacked > limit {
            // files stored so far are removed by the periodic cleanup
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!(
                    "the file unpacks to more than {} MiB of telemetry",
                    limit / (1024 * 1024)
                ),
            ));
        }
        let telemetry = LapTelemetry::from_parquet(bytes.clone())
            .map_err(|e| ApiError::bad_request(format!("telemetry of lap {}: {e}", lap.id)))?;
        lap.sample_count = u32::try_from(telemetry.len()).unwrap_or(u32::MAX);
        state.blobs.put(&bytes)?;
        laps.push((lap, bytes.len() as u64));
    }
    Ok(Bundle { sessions, laps })
}

/// `POST /import`: uploads an `.stintory` file (request body). Sessions and laps
/// the user already has are kept as they are.
pub async fn import(
    State(state): State<SharedState>,
    Auth(session): Auth,
    body: Body,
) -> ApiResult<Json<ImportResult>> {
    // stream the upload into a temporary file (at most STINTORY_MAX_UPLOAD_MB)
    let (tmp, _) = crate::sync::spool_body(&state, body, state.config.upload_limit()).await?;
    let file = tmp.reopen()?;
    // one lap's telemetry at a time is in memory; the stored files must
    // survive until the laps referring to them are committed
    let _slot = crate::sync::decode_slot(&state).await?;
    let _writes = state.blob_writes.read().await;
    let state2 = state.clone();
    let bundle = tokio::task::spawn_blocking(move || read_bundle(&state2, file)).await??;
    drop(tmp);

    let owner = session.user.id;
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    crate::sync::lock_user(&tx, owner, true).await?;
    let mut result = ImportResult {
        files: 1,
        sessions: 0,
        laps: 0,
        skipped_laps: 0,
    };
    for s in &bundle.sessions {
        if insert_session(&tx, owner, s).await? {
            result.sessions += 1;
        }
    }
    for (lap, size) in &bundle.laps {
        if insert_lap(&tx, owner, lap, *size).await? {
            result.laps += 1;
        } else {
            result.skipped_laps += 1;
        }
    }
    tx.commit().await?;
    Ok(Json(result))
}
