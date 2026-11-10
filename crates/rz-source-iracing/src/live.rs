// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Live telemetry from the iRacing memory-mapped file (Windows only).
//!
//! Source: iRacing SDK 1.20, `irsdk_defines.h` (names
//! `Local\IRSDKMemMapFileName` / `Local\IRSDKDataValidEvent`) and the
//! reference client in `irsdk_utils.cpp`:
//!
//! 1. open the file mapping read-only and the data-valid event;
//! 2. the newest row is `varBuf[curBuf]`; it is new if its `tickCount` is
//!    larger than the last one we read;
//! 3. copy the row, then compare `tickCount` with `tickCountBegin`: if they
//!    differ, the sim wrote while we copied, so try again (twice at most);
//! 4. otherwise wait on the event (signalled every tick, 60 Hz).
//!
//! Other cars' `CarIdx*` array entries are zeroed before a row leaves this
//! module (docs/PLAN.md §5.2), and the session info is reduced to the player.

#![allow(unsafe_code)] // FFI and reading shared memory; see SAFETY comments

use std::collections::VecDeque;
use std::sync::atomic::{Ordering, fence};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rz_model::GameId;
use rz_model::capture::{RawRecord, RawSource, SourceError};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Memory::{
    FILE_MAP_READ, MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    OpenFileMappingW, UnmapViewOfFile, VirtualQuery,
};
use windows::Win32::System::Threading::{
    OpenEventW, SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject,
};
use windows::core::w;

use crate::decoder::{data_record, session_info_record, var_table_record};
use crate::format::{HEADER_LEN, Header, VAR_HEADER_LEN, VarTable, latin1};
use crate::session_info::SessionInfo;

/// An open connection to the sim's shared memory.
struct Connection {
    mapping: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    len: usize,
    event: HANDLE,
}

// SAFETY: the handles and the mapped view are process-wide and may be used
// from any thread; we only read through them.
unsafe impl Send for Connection {}

impl Connection {
    /// Opens the mapping and event. `None` if iRacing is not running.
    fn open() -> Option<Self> {
        // SAFETY: plain Win32 calls with valid, static, NUL-terminated names.
        unsafe {
            let mapping =
                OpenFileMappingW(FILE_MAP_READ.0, false, w!("Local\\IRSDKMemMapFileName")).ok()?;
            let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
            if view.Value.is_null() {
                let _ = CloseHandle(mapping);
                return None;
            }
            let mut info = MEMORY_BASIC_INFORMATION::default();
            let written = VirtualQuery(
                Some(view.Value),
                &mut info,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            );
            let event = OpenEventW(
                SYNCHRONIZATION_SYNCHRONIZE,
                false,
                w!("Local\\IRSDKDataValidEvent"),
            );
            let (Ok(event), true) = (event, written != 0) else {
                let _ = UnmapViewOfFile(view);
                let _ = CloseHandle(mapping);
                return None;
            };
            Some(Self {
                mapping,
                view,
                len: info.RegionSize,
                event,
            })
        }
    }

    /// Copies `len` bytes at `offset` out of the shared memory.
    fn copy(&self, offset: usize, len: usize) -> Option<Vec<u8>> {
        let end = offset.checked_add(len)?;
        if end > self.len {
            return None;
        }
        let mut out = vec![0u8; len];
        fence(Ordering::Acquire);
        // SAFETY: the range was checked against the size of the mapped
        // region; the sim may write concurrently, which is why callers
        // validate tick counts before and after (torn-read detection).
        unsafe {
            std::ptr::copy_nonoverlapping(
                (self.view.Value as *const u8).add(offset),
                out.as_mut_ptr(),
                len,
            );
        }
        fence(Ordering::Acquire);
        Some(out)
    }

    fn header(&self) -> Option<Header> {
        Header::parse(&self.copy(0, HEADER_LEN)?).ok()
    }

    /// Waits until the sim signals new data or the timeout expires.
    fn wait(&self, timeout: Duration) {
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: valid event handle owned by self.
        unsafe {
            WaitForSingleObject(self.event, ms);
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: handles/view were created in `open` and are released once.
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.event);
            let _ = CloseHandle(self.mapping);
        }
    }
}

/// [`RawSource`] reading live data from a running iRacing sim.
pub struct LiveSource {
    conn: Option<Connection>,
    pending: VecDeque<RawRecord>,
    last_tick: Option<i32>,
    last_info_update: Option<i32>,
    table_sig: Option<(usize, usize, usize)>,
    table: Option<VarTable>,
    last_session_time: f64,
    session_epoch: Option<f64>,
}

impl std::fmt::Debug for LiveSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveSource")
            .field("connected", &self.is_connected())
            .finish_non_exhaustive()
    }
}

impl Default for LiveSource {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveSource {
    /// Creates a source; it connects lazily when iRacing is running.
    pub fn new() -> Self {
        Self {
            conn: None,
            pending: VecDeque::new(),
            last_tick: None,
            last_info_update: None,
            table_sig: None,
            table: None,
            last_session_time: 0.0,
            session_epoch: None,
        }
    }

    /// True while the sim is running and reports itself connected.
    pub fn is_connected(&self) -> bool {
        self.conn
            .as_ref()
            .and_then(Connection::header)
            .is_some_and(|h| h.is_connected())
    }

    /// Cheap check whether the sim is running and sending telemetry. The
    /// mapping alone is not enough: it outlives the sim session (e.g. while
    /// only the iRacing UI is open, or another tool keeps it open).
    pub fn probe() -> bool {
        Connection::open()
            .and_then(|c| c.header())
            .is_some_and(|h| h.is_connected())
    }

    fn reset(&mut self) {
        self.last_tick = None;
        self.last_info_update = None;
        self.table_sig = None;
        self.table = None;
        self.session_epoch = None;
    }

    /// Reads the newest row if it is newer than the last one.
    fn read_row(&mut self, conn: &Connection, h: &Header) -> Option<Vec<u8>> {
        let latest = usize::from(h.cur_buf).min(h.num_buf.saturating_sub(1));
        let vb = h.var_bufs[latest];
        if self.last_tick.is_some_and(|t| vb.tick_count <= t) {
            if self.last_tick.is_some_and(|t| vb.tick_count < t) {
                self.last_tick = Some(vb.tick_count); // sim restarted
            }
            return None;
        }
        for _ in 0..2 {
            let before = conn.header()?.var_bufs[latest].tick_count;
            let row = conn.copy(vb.buf_offset, h.buf_len)?;
            let after = conn.header()?.var_bufs[latest];
            if after.tick_count_begin == before && after.tick_count == before {
                self.last_tick = Some(before);
                return Some(row);
            }
        }
        None // data changed under us twice; next tick
    }

    fn poll(&mut self, timeout: Duration) -> Result<(), SourceError> {
        if self.conn.is_none() {
            self.conn = Connection::open();
            if self.conn.is_none() {
                std::thread::sleep(timeout.min(Duration::from_secs(1)));
                return Ok(());
            }
        }
        let conn = self.conn.take().expect("checked");
        let result = self.poll_connected(&conn, timeout);
        self.conn = Some(conn);
        result
    }

    fn poll_connected(&mut self, conn: &Connection, timeout: Duration) -> Result<(), SourceError> {
        let Some(mut h) = conn.header() else {
            return Ok(());
        };
        if !h.is_connected() {
            if self.last_tick.is_some() {
                tracing::info!("iRacing disconnected");
                self.reset();
            }
            conn.wait(timeout);
            return Ok(());
        }

        // 1. variable table (fixed during a session, re-read if it changes)
        let sig = (h.num_vars, h.var_header_offset, h.buf_len);
        if self.table_sig != Some(sig) {
            let headers = conn
                .copy(h.var_header_offset, h.num_vars * VAR_HEADER_LEN)
                .ok_or_else(|| SourceError::InvalidData("var headers out of range".into()))?;
            let table = VarTable::parse(&headers, h.num_vars, h.buf_len)
                .map_err(|e| SourceError::InvalidData(e.to_string()))?;
            self.pending.push_back(var_table_record(
                h.tick_rate.max(0) as u32,
                h.buf_len as u32,
                h.num_vars as u32,
                &headers,
            ));
            self.table = Some(table);
            self.table_sig = Some(sig);
            self.last_info_update = None;
        }

        // 2. newest data row (wait for the next tick if there is none)
        let mut row = self.read_row(conn, &h);
        if row.is_none() {
            conn.wait(timeout);
            h = conn.header().unwrap_or(h);
            row = self.read_row(conn, &h);
        }
        let Some(mut row) = row else {
            return Ok(());
        };
        let table = self.table.as_ref().expect("set above");

        // 3. session info (player only) when it changed
        let session_time = table
            .get("SessionTime")
            .and_then(|v| v.read(&row, 0))
            .unwrap_or(0.0);
        if session_time < self.last_session_time - 1.0 {
            self.session_epoch = None; // new session: session time restarted
            self.last_info_update = None;
        }
        self.last_session_time = session_time;
        if self.last_info_update != Some(h.session_info_update)
            && let Some(bytes) = conn.copy(h.session_info_offset, h.session_info_len)
        {
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64();
            let epoch = *self.session_epoch.get_or_insert(now - session_time);
            let info = SessionInfo::from_yaml(&latin1(&bytes[..end]), epoch);
            self.pending.push_back(session_info_record(&info));
            self.last_info_update = Some(h.session_info_update);
        }

        // 4. remove other cars' data, then hand out the row
        let player = table
            .get("PlayerCarIdx")
            .and_then(|v| v.read(&row, 0))
            .map(|v| v as usize);
        table.scrub_other_cars(&mut row, player);
        self.pending.push_back(data_record(row));
        Ok(())
    }
}

impl RawSource for LiveSource {
    fn game(&self) -> GameId {
        GameId::Iracing
    }

    fn next_record(&mut self, timeout: Duration) -> Result<Option<RawRecord>, SourceError> {
        if self.pending.is_empty() {
            self.poll(timeout)?;
        }
        Ok(self.pending.pop_front())
    }
}
