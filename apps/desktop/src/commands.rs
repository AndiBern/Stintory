// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tauri commands called by the frontend (`web/src/lib/api/tauri.ts`).

use std::path::PathBuf;

use rz_analysis::{DEFAULT_STEP_M, MIN_STEP_M};
use rz_capture::pipeline::{self, RunOptions};
use rz_model::api::{
    AppInfo, BackendKind, CompareData, CompareRequest, ConditionsView, ImportProgress,
    ImportResult, LapSummary, RecorderStatus, SessionDetail, SessionSummary, Settings,
};
use rz_model::{LapId, SessionId};
use rz_source_iracing::IbtSource;
use rz_store::{KEEP_RAW_RECORDINGS, SpoolState};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::state::AppState;

/// Event: sessions or laps were added, changed or deleted.
pub const EVENT_SESSIONS_CHANGED: &str = "sessions-changed";
/// Event: import progress ([`ImportProgress`]).
pub const EVENT_IMPORT_PROGRESS: &str = "import-progress";

/// Error returned to the frontend. Serialized as a plain message string.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    /// Store error
    #[error(transparent)]
    Store(#[from] rz_store::StoreError),
    /// Pipeline (import) error
    #[error(transparent)]
    Pipeline(#[from] pipeline::PipelineError),
    /// Analysis error
    #[error(transparent)]
    Analysis(#[from] rz_analysis::AnalysisError),
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
        name: "Rundenzeit".to_owned(),
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
                .ok_or(rz_store::StoreError::SessionNotFound(id))?;
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
    id: String,
) -> CmdResult<()> {
    let id: SessionId = parse_id(&id)?;
    state.with_store(move |s| s.delete_session(id)).await?;
    let _ = app.emit(EVENT_SESSIONS_CHANGED, ());
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
                .collect::<std::result::Result<Vec<_>, rz_store::StoreError>>()
        })
        .await?;
    tauri::async_runtime::spawn_blocking(move || {
        let refs: Vec<_> = laps.iter().map(|(l, t)| (l, t)).collect();
        rz_analysis::compare(&refs, step)
    })
    .await
    .map_err(|e| CommandError::BadRequest(e.to_string()))?
    .map_err(Into::into)
}

/// Imports telemetry files (currently iRacing `.ibt`). Files of the same
/// session are merged; laps already stored are skipped.
#[tauri::command]
pub async fn import_files(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> CmdResult<ImportResult> {
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    if let Some(bad) = paths
        .iter()
        .find(|p| !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ibt")))
    {
        return Err(CommandError::BadRequest(format!(
            "unsupported file: {} (supported: iRacing .ibt)",
            bad.display()
        )));
    }
    let mut sink = state.store.clone();
    let files = paths.len() as u32;
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
    let _ = app.emit(EVENT_SESSIONS_CHANGED, ());
    Ok(ImportResult {
        files,
        sessions: stats.sessions,
        laps: stats.laps,
    })
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
