// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Local storage of the Rundenzeit client.
//!
//! Layout of the data directory:
//!
//! ```text
//! <data>/rundenzeit.db                  SQLite: sessions, laps, blob index
//! <data>/blobs/<aa>/<hash>.parquet      lap telemetry, content addressed
//! <data>/raw/<game>-<id>.rzraw          raw spool files (deleted once processed)
//! ```
//!
//! Sessions and laps can be exported to and imported from `.rzx` files
//! ([`rz_model::rzx`]).
//!
//! Lap telemetry is immutable. Writing a lap first stores its blob (idempotent,
//! atomic rename), then inserts the metadata in one transaction, so a crash
//! never leaves a lap row without its telemetry.

mod blobs;
mod bundle;
mod migrations;
mod settings;
mod spools;
mod store;
mod timefmt;

pub use blobs::BlobStore;
pub use bundle::{BundleImport, ExportOptions, ExportSelection, ExportStats};
pub use settings::{EXPORT_DRIVER_NAME, KEEP_RAW_RECORDINGS};
pub use spools::{RAW_DIR, SpoolEntry, SpoolState};
pub use store::{SAME_LAP_START_TOLERANCE_S, SAME_SESSION_START_TOLERANCE, Store};

use rz_model::{BlobHash, LapId, SessionId};

/// Errors of the local store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// SQLite error
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    /// The database was written by a newer version of Rundenzeit
    #[error(
        "database schema version {found} is newer than supported ({supported}); please update Rundenzeit"
    )]
    SchemaTooNew {
        /// Version found in the database
        found: u32,
        /// Latest version this build knows
        supported: u32,
    },
    /// File system error
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Telemetry encoding/decoding failed
    #[error(transparent)]
    Telemetry(#[from] rz_model::TelemetryError),
    /// JSON column could not be (de)serialized
    #[error("invalid JSON in database: {0}")]
    Json(#[from] serde_json::Error),
    /// A value in the database could not be parsed
    #[error("corrupt value in column {column}: {message}")]
    Corrupt {
        /// Column name
        column: &'static str,
        /// What went wrong
        message: String,
    },
    /// Session does not exist
    #[error("session not found: {0}")]
    SessionNotFound(SessionId),
    /// Lap does not exist
    #[error("lap not found: {0}")]
    LapNotFound(LapId),
    /// Blob file is missing on disk
    #[error("telemetry blob missing: {0}")]
    BlobMissing(BlobHash),
    /// Blob file content does not match its hash
    #[error("telemetry blob corrupt (hash mismatch): {0}")]
    BlobCorrupt(BlobHash),
    /// Reading or writing an `.rzx` file failed
    #[error(transparent)]
    Rzx(#[from] rz_model::rzx::RzxError),
    /// An export without sessions or laps
    #[error("nothing selected to export")]
    NothingToExport,
}

/// Result type of this crate.
pub type Result<T, E = StoreError> = std::result::Result<T, E>;
