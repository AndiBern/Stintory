// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tauri commands called by the frontend (`web/src/lib/api/tauri.ts`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use sy_analysis::{DEFAULT_STEP_M, MIN_STEP_M};
use sy_capture::pipeline::{self, RunOptions};
use sy_model::api::{
    AppInfo, BackendKind, CompareData, CompareRequest, ConditionsView, ExportRequest, ExportResult,
    ImportProgress, ImportResult, LapSummary, RecorderStatus, SessionDetail, SessionSummary,
    Settings, SyncStatus,
};
use sy_model::{LapId, SessionId};
use sy_source_iracing::IbtSource;
use sy_store::{
    EXPORT_DRIVER_NAME, ExportOptions, ExportSelection, KEEP_RAW_RECORDINGS, SpoolState, Store,
    StoreError,
};
use tauri::{AppHandle, Emitter, State};

use crate::state::AppState;
use crate::sync::SyncManager;

/// Event: sessions or laps were added, changed or deleted.
pub const EVENT_SESSIONS_CHANGED: &str = "sessions-changed";
/// Event: import progress ([`ImportProgress`]).
pub const EVENT_IMPORT_PROGRESS: &str = "import-progress";

/// Error returned to the frontend. Serialized as a plain message string.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    /// Store error
    #[error(transparent)]
    Store(#[from] sy_store::StoreError),
    /// Pipeline (import) error
    #[error(transparent)]
    Pipeline(#[from] pipeline::PipelineError),
    /// Analysis error
    #[error(transparent)]
    Analysis(#[from] sy_analysis::AnalysisError),
    /// Bad input from the frontend
    #[error("{0}")]
    BadRequest(String),
}

impl Serialize for CommandError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

type CmdResult<T> = std::result::Result<T, CommandError>;

fn parse_id<T: std::str::FromStr>(s: &str) -> CmdResult<T> {
    s.parse()
        .map_err(|_| CommandError::BadRequest(format!("invalid id: {s}")))
}

/// Basic information about the backend.
#[tauri::command]
pub fn get_info() -> AppInfo {
    AppInfo {
        name: "Stintory".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        backend: BackendKind::Desktop,
    }
}

/// All recorded sessions, newest first.
#[tauri::command]
pub async fn list_sessions(state: State<'_, AppState>) -> CmdResult<Vec<SessionSummary>> {
    Ok(state.with_store(|store| store.list_sessions()).await?)
}

/// A session with its laps.
#[tauri::command]
pub async fn get_session(state: State<'_, AppState>, id: String) -> CmdResult<SessionDetail> {
    let id: SessionId = parse_id(&id)?;
    Ok(state
        .with_store(move |store| {
            let session = store.session(id)?;
            let laps = store.laps(id)?;
            let summary = store
                .list_sessions()?
                .into_iter()
                .find(|s| s.id == id)
                .ok_or(sy_store::StoreError::SessionNotFound(id))?;
            Ok(SessionDetail {
                summary,
                ended_at: session.ended_at,
                game_version: session.game_version,
                conditions: ConditionsView::from(&session.conditions),
                laps: laps.iter().map(LapSummary::from).collect(),
            })
        })
        .await?)
}

/// Deletes a session with all its laps.
#[tauri::command]
pub async fn delete_session(
    app: AppHandle,
    state: State<'_, AppState>,
    sync: State<'_, Arc<SyncManager>>,
    id: String,
) -> CmdResult<()> {
    let id: SessionId = parse_id(&id)?;
    state.with_store(move |s| s.delete_session(id)).await?;
    let _ = app.emit(EVENT_SESSIONS_CHANGED, ());
    sync.nudge();
    Ok(())
}

/// Resamples laps onto a common distance grid for charts (first lap = reference).
#[tauri::command]
pub async fn compare_laps(
    state: State<'_, AppState>,
    request: CompareRequest,
) -> CmdResult<CompareData> {
    if request.lap_ids.is_empty() || request.lap_ids.len() > 8 {
        return Err(CommandError::BadRequest("select 1 to 8 laps".into()));
    }
    let step = request.step_m.unwrap_or(DEFAULT_STEP_M).max(MIN_STEP_M);
    let ids: Vec<LapId> = request.lap_ids;
    let laps = state
        .with_store(move |store| {
            ids.into_iter()
                .map(|id| Ok((store.lap(id)?, store.lap_telemetry(id)?)))
                .collect::<std::result::Result<Vec<_>, sy_store::StoreError>>()
        })
        .await?;
    tauri::async_runtime::spawn_blocking(move || {
        let refs: Vec<_> = laps.iter().map(|(l, t)| (l, t)).collect();
        sy_analysis::compare(&refs, step)
    })
    .await
    .map_err(|e| CommandError::BadRequest(e.to_string()))?
    .map_err(Into::into)
}

fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// Imports files: Stintory exports (`.stintory`) and iRacing telemetry
/// (`.ibt`). `.ibt` files of the same session are merged; laps already
/// stored are skipped.
#[tauri::command]
pub async fn import_files(
    app: AppHandle,
    state: State<'_, AppState>,
    sync: State<'_, Arc<SyncManager>>,
    paths: Vec<String>,
) -> CmdResult<ImportResult> {
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    if let Some(bad) = paths
        .iter()
        .find(|p| !has_extension(p, "ibt") && !has_extension(p, sy_model::bundle::EXTENSION))
    {
        return Err(CommandError::BadRequest(format!(
            "unsupported file: {} (supported: .stintory, iRacing .ibt)",
            bad.display()
        )));
    }
    let (bundles, ibts): (Vec<PathBuf>, Vec<PathBuf>) = paths
        .into_iter()
        .partition(|p| has_extension(p, sy_model::bundle::EXTENSION));
    let mut result = ImportResult {
        files: (bundles.len() + ibts.len()) as u32,
        sessions: 0,
        laps: 0,
        skipped_laps: 0,
    };
    for path in bundles {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let stats = match state.with_store(move |s| s.import_bundle(&path)).await {
            Ok(s) => s,
            Err(e) => {
                // earlier files may have been imported
                let _ = app.emit(EVENT_SESSIONS_CHANGED, ());
                // name the file: several may be imported at once
                return Err(CommandError::BadRequest(format!("{name}: {e}")));
            }
        };
        result.sessions += stats.sessions;
        result.laps += stats.laps;
        result.skipped_laps += stats.skipped_laps;
    }
    if !ibts.is_empty() {
        let stats = import_ibts(&app, &state, ibts).await?;
        result.sessions += stats.sessions;
        result.laps += stats.laps;
    }
    let _ = app.emit(EVENT_SESSIONS_CHANGED, ());
    sync.nudge();
    Ok(result)
}

async fn import_ibts(
    app: &AppHandle,
    state: &State<'_, AppState>,
    paths: Vec<PathBuf>,
) -> CmdResult<pipeline::Stats> {
    let mut sink = state.store.clone();
    let progress_app = app.clone();
    let stats = tauri::async_runtime::spawn_blocking(move || {
        let mut source = IbtSource::open_many(&paths).map_err(pipeline::PipelineError::from)?;
        let total = source.total_rows().max(1) as f32;
        let mut progress = |s: &pipeline::Stats| {
            let _ = progress_app.emit(
                EVENT_IMPORT_PROGRESS,
                ImportProgress {
                    fraction: (s.records as f32 / total).min(1.0),
                    laps: s.laps,
                },
            );
        };
        pipeline::run(
            &mut source,
            &mut sink,
            RunOptions {
                spool: None,
                should_stop: &|| false,
                idle_done: None,
                progress: Some(&mut progress),
            },
        )
    })
    .await
    .map_err(|e| CommandError::BadRequest(e.to_string()))??;
    Ok(stats)
}

/// Writes sessions and/or laps to an `.stintory` file at `path`. Remembers the
/// driver name for the next export.
#[tauri::command]
pub async fn export_bundle(
    state: State<'_, AppState>,
    request: ExportRequest,
    path: String,
) -> CmdResult<ExportResult> {
    let mut path = PathBuf::from(path);
    if !has_extension(&path, sy_model::bundle::EXTENSION) {
        path.set_extension(sy_model::bundle::EXTENSION);
    }
    let driver_name = request
        .driver_name
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty());
    let selection = ExportSelection {
        sessions: request.session_ids,
        laps: request.lap_ids,
    };
    let options = ExportOptions {
        exporter: format!("Stintory {}", env!("CARGO_PKG_VERSION")),
        driver_name,
        include_setup: request.include_setup,
    };
    let shown = path.display().to_string();
    let stats = state
        .with_store(move |s| {
            s.set_setting(EXPORT_DRIVER_NAME, &options.driver_name)?;
            s.export_bundle(&path, &selection, &options)
        })
        .await?;
    Ok(ExportResult {
        path: shown,
        sessions: stats.sessions,
        laps: stats.laps,
    })
}

/// Writes lap telemetry as CSV. One lap: `path` is the file. Several laps:
/// `path` is a directory, the files are named after track, car, date and
/// lap number.
#[tauri::command]
pub async fn export_csv(
    state: State<'_, AppState>,
    lap_ids: Vec<String>,
    path: String,
) -> CmdResult<ExportResult> {
    let ids: Vec<LapId> = lap_ids
        .iter()
        .map(|s| parse_id(s))
        .collect::<CmdResult<_>>()?;
    if ids.is_empty() {
        return Err(CommandError::BadRequest("no laps selected".into()));
    }
    let target = PathBuf::from(&path);
    let shown = target.display().to_string();
    let sessions = state
        .with_store(move |s| {
            let mut sessions = std::collections::BTreeSet::new();
            for &id in &ids {
                let file = if ids.len() == 1 {
                    let mut f = target.clone();
                    if !has_extension(&f, "csv") {
                        f.set_extension("csv");
                    }
                    f
                } else {
                    target.join(csv_file_name(s, id)?)
                };
                let lap = s.lap(id)?;
                sessions.insert(lap.session_id);
                write_csv(s, id, &file)?;
            }
            Ok(sessions.len() as u32)
        })
        .await?;
    Ok(ExportResult {
        path: shown,
        sessions,
        laps: lap_ids.len() as u32,
    })
}

/// `<track>_<car>_<yyyy-mm-dd_hh-mm>_lap<n>.csv`, safe on every OS.
fn csv_file_name(store: &Store, id: LapId) -> Result<String, StoreError> {
    let lap = store.lap(id)?;
    let s = store.session(lap.session_id)?;
    let date = format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}",
        s.started_at.year(),
        u8::from(s.started_at.month()),
        s.started_at.day(),
        s.started_at.hour(),
        s.started_at.minute()
    );
    let name = format!("{}_{}_{date}_lap{}", s.track_name, s.car_name, lap.number);
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok(format!("{safe}.csv"))
}

/// Writes one lap's CSV atomically (temporary file, then rename).
fn write_csv(store: &Store, id: LapId, file: &Path) -> Result<(), StoreError> {
    let telemetry = store.lap_telemetry(id)?;
    let mut tmp = file.as_os_str().to_owned();
    tmp.push(".part");
    let tmp = PathBuf::from(tmp);
    let result = std::fs::File::create(&tmp)
        .and_then(|f| telemetry.write_csv(&f).and_then(|()| f.sync_all()))
        .and_then(|()| std::fs::rename(&tmp, file));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(result?)
}

/// Current settings.
#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> CmdResult<Settings> {
    Ok(state
        .with_store(|store| {
            Ok(Settings {
                data_dir: store.root().display().to_string(),
                keep_raw_recordings: store.setting(KEEP_RAW_RECORDINGS)?.unwrap_or(false),
                raw_storage_bytes: store.spool_size()?,
                failed_recordings: store.spools(&[SpoolState::Failed])?.len() as u32,
                export_driver_name: store.setting(EXPORT_DRIVER_NAME)?.flatten(),
            })
        })
        .await?)
}

/// Debug setting: keep raw recordings after processing.
#[tauri::command]
pub async fn set_keep_raw_recordings(state: State<'_, AppState>, keep: bool) -> CmdResult<()> {
    Ok(state
        .with_store(move |s| s.set_setting(KEEP_RAW_RECORDINGS, &keep))
        .await?)
}

/// Deletes all kept raw recordings (processed or failed; not the one being
/// recorded right now).
#[tauri::command]
pub async fn delete_raw_recordings(state: State<'_, AppState>) -> CmdResult<()> {
    Ok(state
        .with_store(|s| {
            for spool in s.spools(&[SpoolState::Processed, SpoolState::Failed])? {
                s.delete_spool(&spool.id)?;
            }
            Ok(())
        })
        .await?)
}

/// Status of the live recorder.
#[tauri::command]
pub fn recorder_status(state: State<'_, AppState>) -> RecorderStatus {
    state
        .recorder
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

// ----- sync with a server -------------------------------------------------

/// Sync status.
#[tauri::command]
pub fn sync_status(sync: State<'_, Arc<SyncManager>>) -> SyncStatus {
    sync.status()
}

/// Connects to a server through the browser login (waits until done).
#[tauri::command]
pub async fn sync_connect(
    sync: State<'_, Arc<SyncManager>>,
    server_url: String,
) -> Result<SyncStatus, String> {
    sync.inner().clone().connect(&server_url).await
}

/// Stops waiting for the browser login.
#[tauri::command]
pub fn sync_cancel_connect(sync: State<'_, Arc<SyncManager>>) {
    sync.cancel_connect();
}

/// Connects with a device token created on the server's account page.
#[tauri::command]
pub async fn sync_connect_token(
    sync: State<'_, Arc<SyncManager>>,
    server_url: String,
    token: String,
) -> Result<SyncStatus, String> {
    sync.connect_with_token(&server_url, &token).await
}

/// Disconnects from the server (local data stays).
#[tauri::command]
pub async fn sync_disconnect(sync: State<'_, Arc<SyncManager>>) -> Result<SyncStatus, String> {
    sync.disconnect().await
}

/// Syncs now.
#[tauri::command]
pub async fn sync_now(sync: State<'_, Arc<SyncManager>>) -> Result<SyncStatus, String> {
    sync.run().await
}

/// Turns automatic sync on or off.
#[tauri::command]
pub async fn sync_set_auto(
    sync: State<'_, Arc<SyncManager>>,
    auto: bool,
) -> Result<SyncStatus, String> {
    sync.set_auto(auto).await
}

// ----- teams and sharing (on the server) ------------------------------------

/// True for the paths of the server's teams and sharing API (relative to
/// `/api/v1`). Everything else the frontend has no business with.
fn is_sharing_path(path: &str) -> bool {
    let path = path.split('?').next().unwrap_or_default();
    if !path.starts_with('/') || path.contains("..") || path.contains("//") {
        return false;
    }
    let parts: Vec<&str> = path[1..].split('/').collect();
    matches!(
        parts.as_slice(),
        ["teams", ..]
            | ["invites", ..]
            | ["shared", "sessions", ..]
            | ["me", "invites"]
            | ["me", "default-sharing"]
            | ["users", "lookup"]
            | ["users", "suggest"]
            | ["sessions", _, "sharing"]
            | ["laps", _, "sharing"]
    )
}

/// Passes a teams or sharing request on to the connected server, as this
/// device's user. `path` is relative to `/api/v1`.
#[tauri::command]
pub async fn server_request(
    sync: State<'_, Arc<SyncManager>>,
    method: String,
    path: String,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE")
        || !is_sharing_path(&path)
    {
        return Err(format!("request not allowed: {method} {path}"));
    }
    sync.server_request(&method, &path, body.as_ref()).await
}

/// Keeps a session someone shared on the server as a local reference (or
/// refreshes the copy). Returns the id of the local session to open.
#[tauri::command]
pub async fn open_shared_session(
    sync: State<'_, Arc<SyncManager>>,
    owner: String,
    id: String,
) -> Result<String, String> {
    let owner: uuid::Uuid = owner.parse().map_err(|_| format!("invalid id: {owner}"))?;
    let id: SessionId = id.parse().map_err(|_| format!("invalid id: {id}"))?;
    Ok(sync.open_shared(owner, id).await?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_sharing_paths_are_passed_on() {
        for ok in [
            "/teams",
            "/teams/0190/members/0191",
            "/invites/link/abc/accept",
            "/me/invites",
            "/me/default-sharing",
            "/users/lookup?username=max",
            "/users/suggest?q=m",
            "/shared/sessions",
            "/sessions/0190/sharing",
            "/laps/0190/sharing",
        ] {
            assert!(is_sharing_path(ok), "{ok}");
        }
        for bad in [
            "/sessions",
            "/sessions/0190",
            "/me",
            "/me/password",
            "/me/devices",
            "/admin/users",
            "/sync/push",
            "/teams/../me/password",
            "teams",
            "/shared/check",
            "/sessions/0190/sharing/x",
            "//teams",
        ] {
            assert!(!is_sharing_path(bad), "{bad}");
        }
    }
}
