// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The `.stintory` export format: sessions and laps with their telemetry in one
//! file, for moving data between installations and sharing it with others.
//!
//! An `.stintory` file is a ZIP archive:
//!
//! ```text
//! manifest.json        Manifest (format, version, exporter, driver name)
//! sessions/<id>.json   Session metadata (serde JSON of Session)
//! laps/<id>.json       Lap metadata (serde JSON of Lap)
//! laps/<id>.parquet    Lap telemetry, the file format of crate::telemetry
//! ```
//!
//! - `format_version` is `major.minor`. Readers accept every minor version of
//!   their major version and ignore fields they don't know.
//! - The telemetry of a lap is checked against the lap's `blob_hash`
//!   (SHA-256 of the Parquet file).
//! - Telemetry entries are stored uncompressed (Parquet is compressed
//!   already); JSON entries are deflated.

use std::collections::BTreeMap;
use std::io::{Read, Seek, Write};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::ids::{BlobHash, LapId, SessionId};
use crate::session::{Lap, Session};

/// Value of [`Manifest::format`].
pub const FORMAT: &str = "stintory-export";
/// Major version written and read by this implementation.
pub const FORMAT_MAJOR: u32 = 1;
/// Minor version written by this implementation.
pub const FORMAT_MINOR: u32 = 0;
/// File extension (without dot).
pub const EXTENSION: &str = "stintory";

const MANIFEST: &str = "manifest.json";
/// Largest JSON entry we read (guards against ZIP bombs).
const MAX_JSON_BYTES: u64 = 16 << 20;
/// Largest telemetry entry we read.
const MAX_TELEMETRY_BYTES: u64 = 512 << 20;

/// `manifest.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Always [`FORMAT`]
    pub format: String,
    /// `major.minor`
    pub format_version: String,
    /// When the file was written
    #[serde(with = "time::serde::rfc3339")]
    pub exported_at: OffsetDateTime,
    /// Program that wrote the file, e.g. `Stintory 0.1.0`
    pub exporter: String,
    /// Name of the driver as chosen by the exporting user (None = anonymous)
    pub driver_name: Option<String>,
    /// Number of sessions in the file
    pub sessions: u32,
    /// Number of laps in the file
    pub laps: u32,
}

/// Errors reading or writing `.stintory` files.
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    /// I/O error
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Broken ZIP container
    #[error("not a valid .stintory file: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// Broken JSON entry
    #[error("invalid entry {entry}: {source}")]
    Json {
        /// Entry name
        entry: String,
        /// Parse error
        source: serde_json::Error,
    },
    /// No manifest or another format
    #[error("not a Stintory export file")]
    NotAnExport,
    /// Written by a newer, incompatible version
    #[error("export format {0} is not supported (newest supported: {FORMAT_MAJOR}.x)")]
    UnsupportedVersion(String),
    /// An entry the file lists is missing
    #[error("entry {0} missing")]
    Missing(String),
    /// An entry is larger than we accept
    #[error("entry {0} is too large")]
    TooLarge(String),
    /// Telemetry does not match the lap's hash
    #[error("telemetry of lap {0} is damaged (hash mismatch)")]
    HashMismatch(LapId),
    /// A lap refers to a session that is not in the file
    #[error("lap {lap} refers to session {session}, which is not in the file")]
    UnknownSession {
        /// Lap
        lap: LapId,
        /// Its session
        session: SessionId,
    },
}

fn session_entry(id: SessionId) -> String {
    format!("sessions/{id}.json")
}
fn lap_entry(id: LapId) -> String {
    format!("laps/{id}.json")
}
fn telemetry_entry(id: LapId) -> String {
    format!("laps/{id}.parquet")
}

/// Writes an `.stintory` file. Add all sessions, then their laps, then call
/// [`finish`](Self::finish).
pub struct BundleWriter<W: Write + Seek> {
    zip: ZipWriter<W>,
    sessions: Vec<SessionId>,
    laps: u32,
}

impl<W: Write + Seek> std::fmt::Debug for BundleWriter<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleWriter")
            .field("sessions", &self.sessions.len())
            .field("laps", &self.laps)
            .finish_non_exhaustive()
    }
}

impl<W: Write + Seek> BundleWriter<W> {
    /// Starts a new file.
    pub fn new(out: W) -> Self {
        Self {
            zip: ZipWriter::new(out),
            sessions: Vec::new(),
            laps: 0,
        }
    }

    fn json<T: Serialize>(&mut self, name: &str, value: &T) -> Result<(), BundleError> {
        let bytes = serde_json::to_vec_pretty(value).map_err(|source| BundleError::Json {
            entry: name.to_owned(),
            source,
        })?;
        self.zip.start_file(
            name,
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )?;
        self.zip.write_all(&bytes)?;
        Ok(())
    }

    /// Adds a session.
    pub fn add_session(&mut self, session: &Session) -> Result<(), BundleError> {
        self.json(&session_entry(session.id), session)?;
        self.sessions.push(session.id);
        Ok(())
    }

    /// Adds a lap of a session added before, with its telemetry file.
    pub fn add_lap(&mut self, lap: &Lap, telemetry: &[u8]) -> Result<(), BundleError> {
        if !self.sessions.contains(&lap.session_id) {
            return Err(BundleError::UnknownSession {
                lap: lap.id,
                session: lap.session_id,
            });
        }
        if BlobHash::of(telemetry) != lap.blob_hash {
            return Err(BundleError::HashMismatch(lap.id));
        }
        self.json(&lap_entry(lap.id), lap)?;
        self.zip.start_file(
            telemetry_entry(lap.id),
            SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored)
                .large_file(telemetry.len() as u64 >= u64::from(u32::MAX)),
        )?;
        self.zip.write_all(telemetry)?;
        self.laps += 1;
        Ok(())
    }

    /// Writes the manifest and closes the archive.
    pub fn finish(mut self, exporter: &str, driver_name: Option<&str>) -> Result<W, BundleError> {
        let manifest = Manifest {
            format: FORMAT.to_owned(),
            format_version: format!("{FORMAT_MAJOR}.{FORMAT_MINOR}"),
            exported_at: OffsetDateTime::now_utc(),
            exporter: exporter.to_owned(),
            driver_name: driver_name.map(str::to_owned),
            sessions: self.sessions.len() as u32,
            laps: self.laps,
        };
        self.json(MANIFEST, &manifest)?;
        Ok(self.zip.finish()?)
    }
}

/// Reads an `.stintory` file.
pub struct BundleReader<R: Read + Seek> {
    zip: ZipArchive<R>,
    manifest: Manifest,
}

impl<R: Read + Seek> std::fmt::Debug for BundleReader<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleReader")
            .field("manifest", &self.manifest)
            .finish_non_exhaustive()
    }
}

impl<R: Read + Seek> BundleReader<R> {
    /// Opens a file and checks its manifest.
    pub fn open(input: R) -> Result<Self, BundleError> {
        let mut zip = match ZipArchive::new(input) {
            Ok(z) => z,
            Err(zip::result::ZipError::InvalidArchive(_)) => return Err(BundleError::NotAnExport),
            Err(e) => return Err(e.into()),
        };
        let manifest: Manifest = match read_json(&mut zip, MANIFEST) {
            Ok(m) => m,
            Err(BundleError::Missing(_) | BundleError::Json { .. }) => {
                return Err(BundleError::NotAnExport);
            }
            Err(e) => return Err(e),
        };
        if manifest.format != FORMAT {
            return Err(BundleError::NotAnExport);
        }
        let major = manifest
            .format_version
            .split('.')
            .next()
            .and_then(|m| m.parse::<u32>().ok());
        if major != Some(FORMAT_MAJOR) {
            return Err(BundleError::UnsupportedVersion(manifest.format_version));
        }
        Ok(Self { zip, manifest })
    }

    /// The manifest.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn names(&self, prefix: &str, suffix: &str) -> Vec<String> {
        self.zip
            .file_names()
            .filter_map(Result::ok)
            .filter(|n| n.starts_with(prefix) && n.ends_with(suffix))
            .map(|n| n.into_owned())
            .collect()
    }

    /// All sessions, ordered by start time.
    pub fn sessions(&mut self) -> Result<Vec<Session>, BundleError> {
        let mut out: Vec<Session> = self
            .names("sessions/", ".json")
            .into_iter()
            .map(|n| read_json(&mut self.zip, &n))
            .collect::<Result<_, _>>()?;
        out.sort_by_key(|s| s.started_at);
        Ok(out)
    }

    /// All laps, by session and lap number. Every lap's session is in the
    /// file.
    pub fn laps(&mut self) -> Result<Vec<Lap>, BundleError> {
        let sessions: Vec<SessionId> = self
            .names("sessions/", ".json")
            .iter()
            .filter_map(|n| {
                n.strip_prefix("sessions/")?
                    .strip_suffix(".json")?
                    .parse()
                    .ok()
            })
            .collect();
        let mut by_key = BTreeMap::new();
        for name in self.names("laps/", ".json") {
            let lap: Lap = read_json(&mut self.zip, &name)?;
            if !sessions.contains(&lap.session_id) {
                return Err(BundleError::UnknownSession {
                    lap: lap.id,
                    session: lap.session_id,
                });
            }
            by_key.insert((lap.session_id, lap.number, lap.id), lap);
        }
        Ok(by_key.into_values().collect())
    }

    /// The telemetry file (Parquet) of a lap, checked against its hash.
    pub fn telemetry(&mut self, lap: &Lap) -> Result<Vec<u8>, BundleError> {
        let name = telemetry_entry(lap.id);
        let bytes = read_entry(&mut self.zip, &name, MAX_TELEMETRY_BYTES)?;
        if BlobHash::of(&bytes) != lap.blob_hash {
            return Err(BundleError::HashMismatch(lap.id));
        }
        Ok(bytes)
    }
}

fn read_entry<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Result<Vec<u8>, BundleError> {
    let entry = match zip.by_name(name) {
        Ok(e) => e,
        Err(zip::result::ZipError::FileNotFound) => return Err(BundleError::Missing(name.into())),
        Err(e) => return Err(e.into()),
    };
    if entry.size() > limit {
        return Err(BundleError::TooLarge(name.into()));
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    // `take` also guards against entries that lie about their size
    entry.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(BundleError::TooLarge(name.into()));
    }
    Ok(bytes)
}

fn read_json<R: Read + Seek, T: serde::de::DeserializeOwned>(
    zip: &mut ZipArchive<R>,
    name: &str,
) -> Result<T, BundleError> {
    let bytes = read_entry(zip, name, MAX_JSON_BYTES)?;
    serde_json::from_slice(&bytes).map_err(|source| BundleError::Json {
        entry: name.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::session::{Conditions, LapFlags, Origin};
    use crate::{GameId, SessionType};
    use time::macros::datetime;

    fn session() -> Session {
        Session {
            id: SessionId::new(),
            game: GameId::Iracing,
            game_version: None,
            track_key: "iracing:439".into(),
            track_name: "Winton".into(),
            car_key: "iracing:67".into(),
            car_name: "MX-5".into(),
            session_type: SessionType::Practice,
            started_at: datetime!(2026-11-10 16:00 UTC),
            ended_at: None,
            conditions: Conditions::default(),
            setup: None,
            session_info: None,
            origin: Origin::Own,
        }
    }

    fn lap(session: &Session, number: u32, telemetry: &[u8]) -> Lap {
        Lap {
            id: LapId::new(),
            session_id: session.id,
            number,
            lap_time_ms: Some(95_047),
            sector_times_ms: vec![],
            valid: true,
            invalid_reason: None,
            flags: LapFlags::default(),
            blob_hash: BlobHash::of(telemetry),
            sample_rate_hz: 60.0,
            sample_count: 3,
            start_time_s: Some(356.7),
        }
    }

    #[test]
    fn roundtrip() {
        let s = session();
        let (a, b) = (lap(&s, 2, b"two"), lap(&s, 1, b"one"));
        let mut w = BundleWriter::new(Cursor::new(Vec::new()));
        w.add_session(&s).unwrap();
        w.add_lap(&a, b"two").unwrap();
        w.add_lap(&b, b"one").unwrap();
        let file = w.finish("test 1.0", Some("Driver")).unwrap().into_inner();

        let mut r = BundleReader::open(Cursor::new(file)).unwrap();
        assert_eq!(r.manifest().driver_name.as_deref(), Some("Driver"));
        assert_eq!((r.manifest().sessions, r.manifest().laps), (1, 2));
        assert_eq!(r.sessions().unwrap(), vec![s]);
        let laps = r.laps().unwrap();
        assert_eq!(laps, vec![b.clone(), a], "ordered by lap number");
        assert_eq!(r.telemetry(&b).unwrap(), b"one");
    }

    #[test]
    fn rejects_damaged_telemetry_and_foreign_files() {
        let s = session();
        let mut w = BundleWriter::new(Cursor::new(Vec::new()));
        w.add_session(&s).unwrap();
        assert!(matches!(
            w.add_lap(&lap(&s, 1, b"one"), b"other"),
            Err(BundleError::HashMismatch(_))
        ));
        assert!(matches!(
            w.add_lap(&lap(&session(), 1, b"one"), b"one"),
            Err(BundleError::UnknownSession { .. })
        ));

        assert!(matches!(
            BundleReader::open(Cursor::new(b"no zip".to_vec())),
            Err(BundleError::NotAnExport)
        ));
        // a zip without manifest
        let mut z = ZipWriter::new(Cursor::new(Vec::new()));
        z.start_file("x.txt", SimpleFileOptions::default()).unwrap();
        let other = z.finish().unwrap().into_inner();
        assert!(matches!(
            BundleReader::open(Cursor::new(other)),
            Err(BundleError::NotAnExport)
        ));
    }

    #[test]
    fn newer_minor_versions_are_read_newer_major_versions_rejected() {
        let write = |version: &str| {
            let mut z = ZipWriter::new(Cursor::new(Vec::new()));
            z.start_file(MANIFEST, SimpleFileOptions::default())
                .unwrap();
            let m = serde_json::json!({
                "format": FORMAT, "format_version": version,
                "exported_at": "2026-11-10T16:00:00Z", "exporter": "x",
                "driver_name": null, "sessions": 0, "laps": 0,
                "something_new": true
            });
            z.write_all(m.to_string().as_bytes()).unwrap();
            z.finish().unwrap().into_inner()
        };
        assert!(BundleReader::open(Cursor::new(write("1.7"))).is_ok());
        assert!(matches!(
            BundleReader::open(Cursor::new(write("2.0"))),
            Err(BundleError::UnsupportedVersion(v)) if v == "2.0"
        ));
    }
}
