// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Raw spool files (`.syraw`, docs/PLAN.md §5.1).
//!
//! Everything a source reads from the game is appended to a spool file
//! **before** it is parsed. After a crash the file is replayed through the
//! same parser, so nothing is lost. Once a session is fully processed the file
//! is deleted (unless the debug setting "keep raw recordings" is on).
//!
//! # Format (version 1)
//!
//! ```text
//! header (uncompressed):
//!   magic        6 bytes  "SYRAW\0"
//!   version      u16 LE   1
//!   game_len     u8
//!   game         game_len bytes, UTF-8 (GameId::as_str)
//!   created_ms   i64 LE   Unix time in milliseconds
//! body: a sequence of independent zstd frames. Decompressed, the body is a
//! sequence of records:
//!   time_us      u64 LE   microseconds since the spool was created
//!   kind         u8       record kind, defined by the source
//!   len          u32 LE   payload length
//!   payload      len bytes
//! ```
//!
//! Records are buffered and written as one zstd frame at least every
//! [`SpoolWriter::FLUSH_INTERVAL`] or [`SpoolWriter::FLUSH_BYTES`]. A torn
//! last frame (crash during write) is ignored by the reader.
//!
//! The payload format belongs to the source; the spool never interprets it.
//! Sources must filter other drivers' data **before** writing (§5.2).

use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sy_model::GameId;
use time::OffsetDateTime;

const MAGIC: &[u8; 6] = b"SYRAW\0";
const VERSION: u16 = 1;
const ZSTD_LEVEL: i32 = 3;
/// Upper bound for a single record, protects the reader against garbage.
const MAX_RECORD_LEN: u32 = 64 * 1024 * 1024;

/// Errors reading or writing spool files.
#[derive(Debug, thiserror::Error)]
pub enum SpoolError {
    /// I/O error
    #[error(transparent)]
    Io(#[from] io::Error),
    /// Not a spool file or unsupported version
    #[error("invalid spool file: {0}")]
    Invalid(String),
}

/// Header information of a spool file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpoolHeader {
    /// Game the data comes from
    pub game: GameId,
    /// When the spool was created
    pub created_at: OffsetDateTime,
}

/// One raw record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpoolRecord {
    /// Time since the spool was created
    pub time: Duration,
    /// Record kind (defined by the source)
    pub kind: u8,
    /// Raw payload
    pub payload: Vec<u8>,
}

/// Appends records to a spool file.
#[derive(Debug)]
pub struct SpoolWriter {
    file: File,
    path: PathBuf,
    started: Instant,
    buffer: Vec<u8>,
    last_flush: Instant,
}

impl SpoolWriter {
    /// Buffered records are written at least this often.
    pub const FLUSH_INTERVAL: Duration = Duration::from_secs(1);
    /// ... or when this many uncompressed bytes are buffered.
    pub const FLUSH_BYTES: usize = 1024 * 1024;

    /// Creates a new spool file (fails if it exists).
    pub fn create(path: impl Into<PathBuf>, game: GameId) -> Result<Self, SpoolError> {
        let path = path.into();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let game = game.as_str().as_bytes();
        let mut header = Vec::with_capacity(32);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&VERSION.to_le_bytes());
        header.push(u8::try_from(game.len()).expect("game ids are short"));
        header.extend_from_slice(game);
        let created_ms = (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
        header.extend_from_slice(&created_ms.to_le_bytes());
        file.write_all(&header)?;
        file.sync_all()?;
        let now = Instant::now();
        Ok(Self {
            file,
            path,
            started: now,
            buffer: Vec::with_capacity(Self::FLUSH_BYTES),
            last_flush: now,
        })
    }

    /// Path of the spool file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends a record, timestamped now. Writes buffered records to disk
    /// when the flush interval or size is reached.
    pub fn append(&mut self, kind: u8, payload: &[u8]) -> Result<(), SpoolError> {
        let time = self.started.elapsed();
        self.append_at(time, kind, payload)
    }

    /// Appends a record with an explicit timestamp (used by tests and tools).
    pub fn append_at(
        &mut self,
        time: Duration,
        kind: u8,
        payload: &[u8],
    ) -> Result<(), SpoolError> {
        let len = u32::try_from(payload.len())
            .ok()
            .filter(|&l| l <= MAX_RECORD_LEN)
            .ok_or_else(|| SpoolError::Invalid("record too large".into()))?;
        let micros = u64::try_from(time.as_micros()).unwrap_or(u64::MAX);
        self.buffer.extend_from_slice(&micros.to_le_bytes());
        self.buffer.push(kind);
        self.buffer.extend_from_slice(&len.to_le_bytes());
        self.buffer.extend_from_slice(payload);
        if self.buffer.len() >= Self::FLUSH_BYTES
            || self.last_flush.elapsed() >= Self::FLUSH_INTERVAL
        {
            self.flush()?;
        }
        Ok(())
    }

    /// Writes all buffered records as one zstd frame.
    pub fn flush(&mut self) -> Result<(), SpoolError> {
        self.last_flush = Instant::now();
        if self.buffer.is_empty() {
            return Ok(());
        }
        let frame = zstd::bulk::compress(&self.buffer, ZSTD_LEVEL)?;
        self.file.write_all(&frame)?;
        self.file.flush()?;
        self.buffer.clear();
        Ok(())
    }

    /// Flushes and syncs the file to disk, then closes it.
    pub fn finish(mut self) -> Result<PathBuf, SpoolError> {
        self.flush()?;
        self.file.sync_all()?;
        Ok(self.path.clone())
    }
}

impl Drop for SpoolWriter {
    fn drop(&mut self) {
        if let Err(e) = self.flush() {
            tracing::error!(path = %self.path.display(), error = %e, "could not flush spool file");
        }
    }
}

/// Reads records from a spool file.
pub struct SpoolReader {
    header: SpoolHeader,
    decoder: zstd::stream::read::Decoder<'static, BufReader<File>>,
    done: bool,
}

impl std::fmt::Debug for SpoolReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpoolReader")
            .field("header", &self.header)
            .finish_non_exhaustive()
    }
}

impl SpoolReader {
    /// Opens a spool file and reads its header.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SpoolError> {
        let mut file = File::open(path)?;
        let mut magic = [0u8; 6];
        file.read_exact(&mut magic)
            .map_err(|_| SpoolError::Invalid("file too short".into()))?;
        if &magic != MAGIC {
            return Err(SpoolError::Invalid("bad magic".into()));
        }
        let version = u16::from_le_bytes(read_array(&mut file)?);
        if version != VERSION {
            return Err(SpoolError::Invalid(format!(
                "unsupported version {version}"
            )));
        }
        let [game_len] = read_array::<1>(&mut file)?;
        let mut game = vec![0u8; usize::from(game_len)];
        file.read_exact(&mut game)?;
        let game = std::str::from_utf8(&game)
            .ok()
            .and_then(|g| g.parse::<GameId>().ok())
            .ok_or_else(|| SpoolError::Invalid("unknown game".into()))?;
        let created_ms = i64::from_le_bytes(read_array(&mut file)?);
        let created_at =
            OffsetDateTime::from_unix_timestamp_nanos(i128::from(created_ms) * 1_000_000)
                .map_err(|_| SpoolError::Invalid("bad timestamp".into()))?;
        let decoder = zstd::stream::read::Decoder::new(file)?;
        Ok(Self {
            header: SpoolHeader { game, created_at },
            decoder,
            done: false,
        })
    }

    /// The file header.
    pub fn header(&self) -> &SpoolHeader {
        &self.header
    }

    /// Reads the next record. Returns `Ok(None)` at the end of the file,
    /// including a torn last frame after a crash.
    pub fn next_record(&mut self) -> Result<Option<SpoolRecord>, SpoolError> {
        if self.done {
            return Ok(None);
        }
        let mut head = [0u8; 13];
        match read_full(&mut self.decoder, &mut head) {
            Ok(true) => {}
            Ok(false) => return self.end(),
            Err(e) => return self.torn(e),
        }
        let micros = u64::from_le_bytes(head[0..8].try_into().expect("8 bytes"));
        let kind = head[8];
        let len = u32::from_le_bytes(head[9..13].try_into().expect("4 bytes"));
        if len > MAX_RECORD_LEN {
            return Err(SpoolError::Invalid(format!("record length {len}")));
        }
        let mut payload = vec![0u8; len as usize];
        match read_full(&mut self.decoder, &mut payload) {
            Ok(true) => {}
            Ok(false) => return self.end(),
            Err(e) => return self.torn(e),
        }
        Ok(Some(SpoolRecord {
            time: Duration::from_micros(micros),
            kind,
            payload,
        }))
    }

    fn end(&mut self) -> Result<Option<SpoolRecord>, SpoolError> {
        self.done = true;
        Ok(None)
    }

    fn torn(&mut self, e: io::Error) -> Result<Option<SpoolRecord>, SpoolError> {
        tracing::warn!(error = %e, "spool file ends with a torn frame (crash?), ignoring the rest");
        self.end()
    }
}

impl Iterator for SpoolReader {
    type Item = Result<SpoolRecord, SpoolError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_record().transpose()
    }
}

fn read_array<const N: usize>(r: &mut impl Read) -> Result<[u8; N], SpoolError> {
    let mut buf = [0u8; N];
    r.read_exact(&mut buf)
        .map_err(|_| SpoolError::Invalid("truncated header".into()))?;
    Ok(buf)
}

/// Fills `buf` completely. `Ok(false)` if the stream ended before the first
/// byte; a partial read is reported as `UnexpectedEof`.
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> io::Result<bool> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records(n: usize) -> Vec<(Duration, u8, Vec<u8>)> {
        (0..n)
            .map(|i| {
                (
                    Duration::from_micros(i as u64 * 16_667),
                    (i % 3) as u8,
                    vec![i as u8; i % 50],
                )
            })
            .collect()
    }

    #[test]
    fn roundtrip_over_multiple_frames() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.syraw");
        let mut w = SpoolWriter::create(&path, GameId::Iracing).unwrap();
        let input = records(500);
        for (i, (t, k, p)) in input.iter().enumerate() {
            w.append_at(*t, *k, p).unwrap();
            if i % 100 == 0 {
                w.flush().unwrap(); // force several zstd frames
            }
        }
        w.finish().unwrap();

        let mut r = SpoolReader::open(&path).unwrap();
        assert_eq!(r.header().game, GameId::Iracing);
        let output: Vec<_> = r
            .by_ref()
            .map(|rec| {
                let rec = rec.unwrap();
                (rec.time, rec.kind, rec.payload)
            })
            .collect();
        assert_eq!(output, input);
    }

    #[test]
    fn torn_last_frame_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("torn.syraw");
        let mut w = SpoolWriter::create(&path, GameId::Lmu).unwrap();
        for (t, k, p) in records(100) {
            w.append_at(t, k, &p).unwrap();
        }
        w.flush().unwrap();
        for (t, k, p) in records(100) {
            w.append_at(t, k, &p).unwrap();
        }
        w.finish().unwrap();
        // Simulate a crash in the middle of writing the second frame.
        let len = std::fs::metadata(&path).unwrap().len();
        let f = OpenOptions::new().write(true).open(&path).unwrap();
        f.set_len(len - 20).unwrap();

        let r = SpoolReader::open(&path).unwrap();
        let n = r.collect::<Result<Vec<_>, _>>().unwrap().len();
        assert!(
            n >= 100,
            "records of the complete first frame survive, got {n}"
        );
        assert!(n < 200);
    }

    #[test]
    fn unflushed_records_are_written_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drop.syraw");
        {
            let mut w = SpoolWriter::create(&path, GameId::F1_25).unwrap();
            w.append_at(Duration::ZERO, 1, b"hello").unwrap();
        }
        let recs: Vec<_> = SpoolReader::open(&path)
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].payload, b"hello");
    }

    #[test]
    fn rejects_other_files_and_existing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.syraw");
        std::fs::write(&path, b"not a spool file").unwrap();
        assert!(matches!(
            SpoolReader::open(&path),
            Err(SpoolError::Invalid(_))
        ));
        assert!(SpoolWriter::create(&path, GameId::Iracing).is_err());
    }
}
