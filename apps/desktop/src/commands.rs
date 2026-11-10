// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tauri commands called by the frontend (`web/src/lib/api/tauri.ts`).

use std::path::{Path, PathBuf};

use rz_analysis::{DEFAULT_STEP_M, MIN_STEP_M};
use rz_capture::pipeline::{self, RunOptions};
use rz_model::api::{
    AppInfo, BackendKind, CompareData, CompareRequest, ConditionsView, ExportRequest, ExportResult,
    ImportProgress, ImportResult, LapSummary, RecorderStatus, SessionDetail, SessionSummary,
    Settings,
};
use rz_model::{LapId, SessionId};
use rz_source_iracing::IbtSource;
use rz_store::{
    EXPORT_DRIVER_NAME, ExportOptions, ExportSelection, KEEP_RAW_RECORDINGS, SpoolState, Store,
    StoreError,
};
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

fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// Imports files: Rundenzeit exports (`.rzx`) and iRacing telemetry
/// (`.ibt`). `.ibt` files of the same session are merged; laps already
/// stored are skipped.
#[tauri::command]
pub async fn import_files(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> CmdResult<ImportResult> {
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    if let Some(bad) = paths
        .iter()
        .find(|p| !has_extension(p, "ibt") && !has_extension(p, rz_model::rzx::EXTENSION))
    {
        return Err(CommandError::BadRequest(format!(
            "unsupported file: {} (supported: Rundenzeit .rzx, iRacing .ibt)",
            bad.display()
        )));
    }
    let (bundles, ibts): (Vec<PathBuf>, Vec<PathBuf>) = paths
        .into_iter()
        .partition(|p| has_extension(p, rz_model::rzx::EXTENSION));
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
        let stats = match state.with_store(move |s| s.import_rzx(&path)).await {
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

/// Writes sessions and/or laps to an `.rzx` file at `path`. Remembers the
/// driver name for the next export.
#[tauri::command]
pub async fn export_rzx(
    state: State<'_, AppState>,
    request: ExportRequest,
    path: String,
) -> CmdResult<ExportResult> {
    let mut path = PathBuf::from(path);
    if !has_extension(&path, rz_model::rzx::EXTENSION) {
        path.set_extension(rz_model::rzx::EXTENSION);
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
        exporter: format!("Rundenzeit {}", env!("CARGO_PKG_VERSION")),
        driver_name,
        include_setup: request.include_setup,
    };
    let shown = path.display().to_string();
    let stats = state
        .with_store(move |s| {
            s.set_setting(EXPORT_DRIVER_NAME, &options.driver_name)?;
            s.export_rzx(&path, &selection, &options)
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
