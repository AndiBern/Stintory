// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `.ibt` telemetry files.
//!
//! Source: iRacing SDK 1.20, `irsdk_defines.h` ("Logged data") and the
//! reference reader `irsdk_diskclient.cpp`: an `irsdk_header`, directly
//! followed by an `irsdk_diskSubHeader`; the header's offsets point to the
//! session info string and the var headers; data rows (`bufLen` bytes each)
//! follow back to back from `varBuf[0].bufOffset`.
//!
//! iRacing may split one session into several files (observed: a new file
//! starts mid-lap where the previous one ended). [`IbtSource::open_many`]
//! orders files by recording time so they decode as one continuous stream.
//!
//! `.ibt` files only contain the player's car, so rows need no filtering;
//! the session info is reduced to the player like everywhere else.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sy_model::GameId;
use sy_model::capture::{RawRecord, RawSource, SourceError};

use crate::decoder::{data_record, session_info_record, var_table_record};
use crate::format::{
    DISK_SUB_HEADER_LEN, DiskSubHeader, HEADER_LEN, Header, VAR_HEADER_LEN, VarTable, latin1,
};
use crate::session_info::SessionInfo;

/// An opened `.ibt` file (header, var table and session info read).
#[derive(Debug)]
pub struct IbtFile {
    path: PathBuf,
    header: Header,
    sub: DiskSubHeader,
    var_headers: Vec<u8>,
    info: SessionInfo,
    rows: usize,
    reader: BufReader<File>,
    next_row: usize,
}

fn invalid(path: &Path, msg: impl std::fmt::Display) -> SourceError {
    SourceError::InvalidData(format!("{}: {msg}", path.display()))
}

impl IbtFile {
    /// Opens a file and reads everything except the data rows.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SourceError> {
        let path = path.as_ref().to_path_buf();
        let mut file = File::open(&path)?;
        let file_len = file.metadata()?.len() as usize;

        let mut head = vec![0u8; HEADER_LEN + DISK_SUB_HEADER_LEN];
        file.read_exact(&mut head)
            .map_err(|_| invalid(&path, "file too short"))?;
        let header = Header::parse(&head).map_err(|e| invalid(&path, e))?;
        let sub = DiskSubHeader::parse(&head[HEADER_LEN..]).map_err(|e| invalid(&path, e))?;
        if header.buf_len == 0 || header.num_vars == 0 {
            return Err(invalid(&path, "no variables"));
        }

        let mut var_headers = vec![0u8; header.num_vars * VAR_HEADER_LEN];
        file.seek(SeekFrom::Start(header.var_header_offset as u64))?;
        file.read_exact(&mut var_headers)
            .map_err(|_| invalid(&path, "truncated var headers"))?;
        // validate early
        VarTable::parse(&var_headers, header.num_vars, header.buf_len)
            .map_err(|e| invalid(&path, e))?;

        let mut info_bytes = vec![0u8; header.session_info_len];
        file.seek(SeekFrom::Start(header.session_info_offset as u64))?;
        file.read_exact(&mut info_bytes)
            .map_err(|_| invalid(&path, "truncated session info"))?;
        let end = info_bytes
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(info_bytes.len());
        // session time 0 in wall-clock time
        let epoch = sub.session_start_date as f64 - sub.session_start_time;
        let info = SessionInfo::from_yaml(&latin1(&info_bytes[..end]), epoch);

        let data_offset = header.var_bufs[0].buf_offset;
        // Trust the file size over the record count (an unfinished file may
        // have a stale count).
        let rows = file_len.saturating_sub(data_offset) / header.buf_len;
        file.seek(SeekFrom::Start(data_offset as u64))?;
        Ok(Self {
            path,
            header,
            sub,
            var_headers,
            info,
            rows,
            reader: BufReader::with_capacity(1 << 20, file),
            next_row: 0,
        })
    }

    /// Path of the file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Player-only session info.
    pub fn info(&self) -> &SessionInfo {
        &self.info
    }

    /// Number of data rows.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Session time of the first row (for ordering split files).
    pub fn start_session_time(&self) -> f64 {
        self.sub.session_start_time
    }

    /// The variable table of this file.
    pub fn var_table(&self) -> VarTable {
        VarTable::parse(&self.var_headers, self.header.num_vars, self.header.buf_len)
            .expect("validated in open")
    }

    fn var_table_record(&self) -> RawRecord {
        var_table_record(
            self.header.tick_rate.max(0) as u32,
            self.header.buf_len as u32,
            self.header.num_vars as u32,
            &self.var_headers,
        )
    }

    /// Reads the next data row.
    pub fn next_row(&mut self) -> Result<Option<Vec<u8>>, SourceError> {
        if self.next_row >= self.rows {
            return Ok(None);
        }
        let mut row = vec![0u8; self.header.buf_len];
        self.reader.read_exact(&mut row)?;
        self.next_row += 1;
        Ok(Some(row))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    VarTable,
    SessionInfo,
    Rows,
}

/// A [`RawSource`] over one or more `.ibt` files.
#[derive(Debug)]
pub struct IbtSource {
    files: Vec<IbtFile>,
    current: usize,
    stage: Stage,
}

impl IbtSource {
    /// Opens several files and orders them by session start and session
    /// time, so files of the same session are read consecutively.
    pub fn open_many<P: AsRef<Path>>(paths: &[P]) -> Result<Self, SourceError> {
        let mut files = paths
            .iter()
            .map(IbtFile::open)
            .collect::<Result<Vec<_>, _>>()?;
        files.sort_by(|a, b| {
            let key = |f: &IbtFile| {
                (
                    (f.info.session_epoch_unix / 60.0).round() as i64,
                    f.info.event.sub_session_id.unwrap_or(0),
                )
            };
            key(a)
                .cmp(&key(b))
                .then(a.start_session_time().total_cmp(&b.start_session_time()))
        });
        Ok(Self {
            files,
            current: 0,
            stage: Stage::VarTable,
        })
    }

    /// Opens a single file.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SourceError> {
        Self::open_many(&[path])
    }

    /// The files in reading order.
    pub fn files(&self) -> &[IbtFile] {
        &self.files
    }

    /// Total number of data rows (for progress reporting).
    pub fn total_rows(&self) -> usize {
        self.files.iter().map(IbtFile::rows).sum()
    }
}

impl RawSource for IbtSource {
    fn game(&self) -> GameId {
        GameId::Iracing
    }

    fn next_record(&mut self, _timeout: Duration) -> Result<Option<RawRecord>, SourceError> {
        loop {
            let Some(file) = self.files.get_mut(self.current) else {
                return Ok(None);
            };
            match self.stage {
                Stage::VarTable => {
                    self.stage = Stage::SessionInfo;
                    return Ok(Some(file.var_table_record()));
                }
                Stage::SessionInfo => {
                    self.stage = Stage::Rows;
                    return Ok(Some(session_info_record(file.info())));
                }
                Stage::Rows => match file.next_row()? {
                    Some(row) => return Ok(Some(data_record(row))),
                    None => {
                        self.current += 1;
                        self.stage = Stage::VarTable;
                    }
                },
            }
        }
    }
}
