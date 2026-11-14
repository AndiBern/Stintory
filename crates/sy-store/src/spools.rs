// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Bookkeeping of raw spool files (docs/ARCHITECTURE.md §5.1).
//!
//! The files themselves are written by `sy-capture`. The store records each
//! file's state so that unprocessed files can be replayed after a crash and
//! processed files can be deleted (or kept, with the debug setting).

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use rusqlite::{OptionalExtension, params};
use sy_model::GameId;
use time::OffsetDateTime;

use crate::store::Store;
use crate::{Result, StoreError, timefmt};

/// Directory for spool files, relative to the data directory.
pub const RAW_DIR: &str = "raw";

/// State of a spool file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpoolState {
    /// Being written, or written but not completely processed (after a crash).
    Recording,
    /// All laps from it are stored.
    Processed,
    /// Processing failed; kept for a retry after an update.
    Failed,
}

impl SpoolState {
    fn as_str(self) -> &'static str {
        match self {
            SpoolState::Recording => "recording",
            SpoolState::Processed => "processed",
            SpoolState::Failed => "failed",
        }
    }
}

impl fmt::Display for SpoolState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SpoolState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "recording" => Ok(SpoolState::Recording),
            "processed" => Ok(SpoolState::Processed),
            "failed" => Ok(SpoolState::Failed),
            other => Err(format!("unknown spool state {other}")),
        }
    }
}

/// A spool file known to the store.
#[derive(Debug, Clone, PartialEq)]
pub struct SpoolEntry {
    /// Id (also the file stem)
    pub id: String,
    /// Game
    pub game: GameId,
    /// Absolute path of the file
    pub path: PathBuf,
    /// State
    pub state: SpoolState,
    /// Error message if failed
    pub error: Option<String>,
    /// When the spool was registered
    pub created_at: OffsetDateTime,
}

impl Store {
    /// Directory where spool files live (created on demand).
    pub fn raw_dir(&self) -> Result<PathBuf> {
        let dir = self.root().join(RAW_DIR);
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    /// Registers a new spool file in state `Recording` and returns the path
    /// to create it at: `<data>/raw/<game>-<id>.syraw`.
    pub fn register_spool(&mut self, game: GameId) -> Result<SpoolEntry> {
        let id = uuid_v7_string();
        let relative = Path::new(RAW_DIR).join(format!("{game}-{id}.syraw"));
        self.raw_dir()?;
        let now = OffsetDateTime::now_utc();
        self.conn().execute(
            "INSERT INTO raw_spools (id, game, path, state, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'recording', ?4, ?4)",
            params![
                id,
                game.as_str(),
                path_to_db(&relative),
                timefmt::encode(now)
            ],
        )?;
        Ok(SpoolEntry {
            id,
            game,
            path: self.root().join(relative),
            state: SpoolState::Recording,
            error: None,
            created_at: now,
        })
    }

    /// Sets the state (and error message) of a spool.
    pub fn set_spool_state(
        &mut self,
        id: &str,
        state: SpoolState,
        error: Option<&str>,
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE raw_spools SET state = ?2, error = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, state.as_str(), error, timefmt::now()],
        )?;
        Ok(())
    }

    /// Spools in one of the given states, oldest first.
    pub fn spools(&self, states: &[SpoolState]) -> Result<Vec<SpoolEntry>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, game, path, state, error, created_at FROM raw_spools ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, game, path, state, error, created_at) = row?;
            let state: SpoolState = state.parse().map_err(|message| StoreError::Corrupt {
                column: "raw_spools.state",
                message,
            })?;
            if !states.contains(&state) {
                continue;
            }
            out.push(SpoolEntry {
                id,
                game: game.parse().map_err(|e: sy_model::game::UnknownGame| {
                    StoreError::Corrupt {
                        column: "raw_spools.game",
                        message: e.to_string(),
                    }
                })?,
                path: self.root().join(path),
                state,
                error,
                created_at: timefmt::decode("raw_spools.created_at", &created_at)?,
            });
        }
        Ok(out)
    }

    /// Deletes a spool's file and its row.
    pub fn delete_spool(&mut self, id: &str) -> Result<()> {
        let path: Option<String> = self
            .conn()
            .query_row("SELECT path FROM raw_spools WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(path) = path {
            match std::fs::remove_file(self.root().join(&path)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            self.conn()
                .execute("DELETE FROM raw_spools WHERE id = ?1", [id])?;
        }
        Ok(())
    }

    /// Total size in bytes of all spool files on disk.
    pub fn spool_size(&self) -> Result<u64> {
        let mut total = 0;
        for entry in self.spools(&[
            SpoolState::Recording,
            SpoolState::Processed,
            SpoolState::Failed,
        ])? {
            total += std::fs::metadata(&entry.path).map(|m| m.len()).unwrap_or(0);
        }
        Ok(total)
    }
}

fn uuid_v7_string() -> String {
    // Reuse the model's UUIDv7 generator for time-ordered, unique ids.
    sy_model::LapId::new().to_string()
}

/// Stores paths with forward slashes so the database is portable.
fn path_to_db(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spool_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let a = store.register_spool(GameId::Iracing).unwrap();
        let b = store.register_spool(GameId::Iracing).unwrap();
        assert!(a.path.starts_with(dir.path().join(RAW_DIR)));
        assert!(a.path.to_string_lossy().ends_with(".syraw"));
        std::fs::write(&a.path, b"data").unwrap();

        store
            .set_spool_state(&b.id, SpoolState::Failed, Some("bad header"))
            .unwrap();
        let recording = store.spools(&[SpoolState::Recording]).unwrap();
        assert_eq!(recording.len(), 1);
        assert_eq!(recording[0].id, a.id);
        let failed = store.spools(&[SpoolState::Failed]).unwrap();
        assert_eq!(failed[0].error.as_deref(), Some("bad header"));
        assert_eq!(store.spool_size().unwrap(), 4);

        store.delete_spool(&a.id).unwrap();
        assert!(!a.path.exists());
        assert!(store.spools(&[SpoolState::Recording]).unwrap().is_empty());
        store.delete_spool(&b.id).unwrap(); // file never existed: fine
    }
}
