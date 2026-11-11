// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The processing pipeline shared by live recording, file import and crash
//! recovery (docs/PLAN.md §5):
//!
//! ```text
//! RawSource ─► [spool] ─► Decoder ─► Recorder ─► Sink (store)
//! ```

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sy_model::GameId;
use sy_model::capture::{Decoder, RawSource, SourceError};
use sy_source_f1::F1Decoder;
use sy_source_iracing::IracingDecoder;
use sy_store::{KEEP_RAW_RECORDINGS, SpoolEntry, SpoolState, Store, StoreError};

use crate::recorder::{Recorder, RecorderConfig, RecorderOutput};
use crate::spool::{SpoolError, SpoolReader, SpoolWriter};

/// Errors of the pipeline.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// Reading or decoding game data failed
    #[error(transparent)]
    Source(#[from] SourceError),
    /// Storing failed
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Spool file error
    #[error(transparent)]
    Spool(#[from] SpoolError),
    /// No decoder for this game yet
    #[error("game {0} is not supported yet")]
    Unsupported(GameId),
    /// Stopped on request
    #[error("cancelled")]
    Cancelled,
}

/// Where recorder output goes (normally the store).
pub trait Sink {
    /// Persists one output.
    fn apply(&mut self, output: RecorderOutput) -> Result<(), StoreError>;
}

impl Sink for Store {
    fn apply(&mut self, output: RecorderOutput) -> Result<(), StoreError> {
        match output {
            RecorderOutput::SessionStarted(s)
            | RecorderOutput::SessionUpdated(s)
            | RecorderOutput::SessionEnded(s) => self.upsert_session(&s).map(|_| ()),
            RecorderOutput::Lap(lap) => self.insert_lap(lap.lap, &lap.telemetry).map(|_| ()),
        }
    }
}

/// A store shared with the UI; locked per output, so long imports don't
/// block other queries.
#[derive(Debug, Clone)]
pub struct SharedStore(pub Arc<Mutex<Store>>);

impl SharedStore {
    /// Runs `f` with the locked store.
    pub fn with<T>(&self, f: impl FnOnce(&mut Store) -> T) -> T {
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }
}

impl Sink for SharedStore {
    fn apply(&mut self, output: RecorderOutput) -> Result<(), StoreError> {
        self.with(|s| s.apply(output))
    }
}

/// Counts what a pipeline run produced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Raw records processed
    pub records: u64,
    /// Sessions started
    pub sessions: u32,
    /// Laps stored
    pub laps: u32,
}

/// The decoder for a game, if supported.
pub fn decoder_for(game: GameId) -> Option<Box<dyn Decoder + Send>> {
    match game {
        GameId::Iracing => Some(Box::new(IracingDecoder::new())),
        GameId::F1_25 => Some(Box::new(F1Decoder::new())),
        GameId::Lmu => None,
    }
}

/// Recorder settings for a game.
pub fn recorder_config(game: GameId) -> RecorderConfig {
    match game {
        // the official lap time arrives up to ~2.2 s after the line
        GameId::Iracing => RecorderConfig {
            official_time_wait_s: 8.0,
            ..RecorderConfig::default()
        },
        _ => RecorderConfig::default(),
    }
}

/// Feeds events from `decoder` through `recorder` into `sink`.
fn emit(
    events: Vec<sy_model::capture::SourceEvent>,
    recorder: &mut Recorder,
    sink: &mut dyn Sink,
    stats: &mut Stats,
) -> Result<(), StoreError> {
    for event in events {
        for out in recorder.handle(event) {
            count(&out, stats);
            sink.apply(out)?;
        }
    }
    Ok(())
}

fn count(out: &RecorderOutput, stats: &mut Stats) {
    match out {
        RecorderOutput::SessionStarted(_) => stats.sessions += 1,
        RecorderOutput::Lap(_) => stats.laps += 1,
        _ => {}
    }
}

/// Options of [`run`].
pub struct RunOptions<'a> {
    /// Write every raw record to this spool first (live recording).
    pub spool: Option<&'a mut SpoolWriter>,
    /// Polled between records; return true to stop.
    pub should_stop: &'a dyn Fn() -> bool,
    /// Return `true` when a live source is done (game closed). Called when
    /// no record arrived within the poll timeout. File sources end on their
    /// own when they return `None`.
    pub idle_done: Option<&'a mut dyn FnMut(Duration) -> bool>,
    /// Called every ~1000 records with the running stats.
    pub progress: Option<&'a mut dyn FnMut(&Stats)>,
}

/// Runs a source through decoder and recorder into the sink until the source
/// ends (file) or `idle_done` says so (live). Always finishes the session.
pub fn run(
    source: &mut dyn RawSource,
    sink: &mut dyn Sink,
    mut opts: RunOptions<'_>,
) -> Result<Stats, PipelineError> {
    let game = source.game();
    let mut decoder = decoder_for(game).ok_or(PipelineError::Unsupported(game))?;
    let mut recorder = Recorder::new(recorder_config(game));
    let mut stats = Stats::default();
    let poll = Duration::from_millis(200);
    let mut idle_since: Option<Instant> = None;

    let result = loop {
        if (opts.should_stop)() {
            break Err(PipelineError::Cancelled);
        }
        let record = match source.next_record(poll) {
            Ok(r) => r,
            Err(e) => break Err(e.into()),
        };
        let Some(record) = record else {
            match opts.idle_done.as_mut() {
                None => break Ok(()), // file source: end of input
                Some(done) => {
                    let idle = idle_since.get_or_insert_with(Instant::now).elapsed();
                    if done(idle) {
                        break Ok(());
                    }
                    continue;
                }
            }
        };
        idle_since = None;
        if let Some(spool) = opts.spool.as_deref_mut()
            && let Err(e) = spool.append(record.kind, &record.payload)
        {
            break Err(e.into());
        }
        stats.records += 1;
        let events = match decoder.decode(&record) {
            Ok(e) => e,
            Err(e) => break Err(e.into()),
        };
        if let Err(e) = emit(events, &mut recorder, sink, &mut stats) {
            break Err(e.into());
        }
        if stats.records % 1000 == 0
            && let Some(p) = opts.progress.as_deref_mut()
        {
            p(&stats);
        }
    };

    // Always close the session, also after errors or cancel: the laps so far
    // are valid data.
    let finish = decoder.finish();
    emit(finish, &mut recorder, sink, &mut stats)?;
    for out in recorder.finish() {
        count(&out, &mut stats);
        sink.apply(out)?;
    }
    if let Some(p) = opts.progress.as_deref_mut() {
        p(&stats);
    }
    result.map(|()| stats)
}

/// [`RawSource`] over a spool file (crash recovery, debugging).
pub struct SpoolSource {
    reader: SpoolReader,
}

impl SpoolSource {
    /// Opens a spool file.
    pub fn open(path: &std::path::Path) -> Result<Self, SpoolError> {
        Ok(Self {
            reader: SpoolReader::open(path)?,
        })
    }
}

impl RawSource for SpoolSource {
    fn game(&self) -> GameId {
        self.reader.header().game
    }

    fn next_record(
        &mut self,
        _timeout: Duration,
    ) -> Result<Option<sy_model::capture::RawRecord>, SourceError> {
        match self.reader.next_record() {
            Ok(r) => Ok(r.map(|r| sy_model::capture::RawRecord {
                kind: r.kind,
                payload: r.payload,
            })),
            Err(SpoolError::Io(e)) => Err(SourceError::Io(e)),
            Err(SpoolError::Invalid(m)) => Err(SourceError::InvalidData(m)),
        }
    }
}

/// Several spool files of one game read as one stream, in the given order
/// (e.g. a recording that was split in two). Sessions are told apart by the
/// game's data, so unrelated files simply give several sessions.
pub struct SpoolChain {
    game: GameId,
    files: std::collections::VecDeque<SpoolSource>,
}

impl SpoolChain {
    /// Opens the files; all must be of the same game.
    pub fn open(paths: &[std::path::PathBuf]) -> Result<Self, SpoolError> {
        let files: std::collections::VecDeque<SpoolSource> = paths
            .iter()
            .map(|p| SpoolSource::open(p))
            .collect::<Result<_, _>>()?;
        let game = files
            .front()
            .map(RawSource::game)
            .ok_or_else(|| SpoolError::Invalid("no files".into()))?;
        if files.iter().any(|f| f.game() != game) {
            return Err(SpoolError::Invalid(
                "the raw recordings are of different games".into(),
            ));
        }
        Ok(Self { game, files })
    }
}

impl RawSource for SpoolChain {
    fn game(&self) -> GameId {
        self.game
    }

    fn next_record(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<sy_model::capture::RawRecord>, SourceError> {
        while let Some(file) = self.files.front_mut() {
            if let Some(r) = file.next_record(timeout)? {
                return Ok(Some(r));
            }
            self.files.pop_front();
        }
        Ok(None)
    }
}

/// Marks a finished spool as processed and deletes the file unless the
/// debug setting "keep raw recordings" is on. A spool without any records
/// (the game was running, but sent no data) is always deleted.
pub fn finish_spool(
    store: &SharedStore,
    spool: &SpoolEntry,
    stats: &Stats,
) -> Result<(), StoreError> {
    store.with(|s| {
        let keep = s.setting::<bool>(KEEP_RAW_RECORDINGS)?.unwrap_or(false);
        if keep && stats.records > 0 {
            s.set_spool_state(&spool.id, SpoolState::Processed, None)
        } else {
            s.delete_spool(&spool.id)
        }
    })
}

/// Crash recovery: replays every spool still in state `Recording` (the app
/// stopped while recording). Laps already stored are skipped (derived ids).
/// Returns the number of recovered laps.
pub fn recover_spools(store: &SharedStore) -> Result<u32, StoreError> {
    let pending = store.with(|s| s.spools(&[SpoolState::Recording]))?;
    let mut laps = 0;
    for spool in pending {
        tracing::info!(path = %spool.path.display(), "recovering unprocessed recording");
        let mut sink = store.clone();
        let result = SpoolSource::open(&spool.path)
            .map_err(PipelineError::from)
            .and_then(|mut src| {
                run(
                    &mut src,
                    &mut sink,
                    RunOptions {
                        spool: None,
                        should_stop: &|| false,
                        idle_done: None,
                        progress: None,
                    },
                )
            });
        match result {
            Ok(stats) => {
                laps += stats.laps;
                finish_spool(store, &spool, &stats)?;
            }
            Err(e) => {
                tracing::error!(error = %e, path = %spool.path.display(), "recovery failed");
                let msg = e.to_string();
                store.with(|s| s.set_spool_state(&spool.id, SpoolState::Failed, Some(&msg)))?;
            }
        }
    }
    Ok(laps)
}
