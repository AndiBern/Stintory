// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The iRacing telemetry binary format, shared by the live memory-mapped
//! file and `.ibt` files.
//!
//! Source: iRacing SDK 1.20, `irsdk_defines.h` (structs `irsdk_header`,
//! `irsdk_varBuf`, `irsdk_varHeader`, `irsdk_diskSubHeader`, enum
//! `irsdk_VarType`). All values are little-endian. Variables in a data row
//! are packed without alignment, so every read is unaligned.
//!
//! Layout (sizes verified against recorded `.ibt` files):
//! - header: 112 bytes (11 × i32, u8 `curBuf`, 3 pad, 4 × varBuf of 16 bytes)
//! - disk sub header (`.ibt` only, directly after the header): 32 bytes
//! - var header: 144 bytes each (type, offset, count, countAsTime + 3 pad,
//!   name[32], desc[64], unit[32])

use std::collections::HashMap;

/// Size of `irsdk_header`.
pub const HEADER_LEN: usize = 112;
/// Size of `irsdk_diskSubHeader`.
pub const DISK_SUB_HEADER_LEN: usize = 32;
/// Size of `irsdk_varHeader`.
pub const VAR_HEADER_LEN: usize = 144;
/// `IRSDK_MAX_BUFS`
pub const MAX_BUFS: usize = 4;
/// `irsdk_stConnected` bit of the header status field.
pub const STATUS_CONNECTED: i32 = 1;

/// Errors decoding the binary format.
#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    /// The data is shorter than the structure requires.
    #[error("truncated {0}")]
    Truncated(&'static str),
    /// A field has an impossible value.
    #[error("invalid {field}: {value}")]
    Invalid {
        /// Field name
        field: &'static str,
        /// The offending value
        value: i64,
    },
}

fn i32_at(b: &[u8], off: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

fn f64_at(b: &[u8], off: usize) -> Option<f64> {
    Some(f64::from_le_bytes(b.get(off..off + 8)?.try_into().ok()?))
}

fn non_negative(field: &'static str, v: i32) -> Result<usize, FormatError> {
    usize::try_from(v).map_err(|_| FormatError::Invalid {
        field,
        value: i64::from(v),
    })
}

/// `irsdk_varBuf`: one of the rotating data buffers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VarBuf {
    /// Updated after a write completes.
    pub tick_count: i32,
    /// Offset of the buffer from the start of the memory/file.
    pub buf_offset: usize,
    /// Updated before a write starts (torn-read detection).
    pub tick_count_begin: i32,
}

/// `irsdk_header`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// API version (`IRSDK_VER`, currently 2)
    pub ver: i32,
    /// Status bits (`STATUS_CONNECTED`)
    pub status: i32,
    /// Data rows per second (60)
    pub tick_rate: i32,
    /// Incremented when the session info string changes
    pub session_info_update: i32,
    /// Length of the session info string
    pub session_info_len: usize,
    /// Offset of the session info string
    pub session_info_offset: usize,
    /// Number of variables
    pub num_vars: usize,
    /// Offset of the var header array
    pub var_header_offset: usize,
    /// Number of data buffers in use
    pub num_buf: usize,
    /// Length of one data row
    pub buf_len: usize,
    /// Index of the most recently written buffer
    pub cur_buf: u8,
    /// The data buffers
    pub var_bufs: [VarBuf; MAX_BUFS],
}

impl Header {
    /// Parses the header from the start of `b`.
    pub fn parse(b: &[u8]) -> Result<Self, FormatError> {
        if b.len() < HEADER_LEN {
            return Err(FormatError::Truncated("header"));
        }
        let i = |n: usize| i32_at(b, n * 4).expect("length checked");
        let mut var_bufs = [VarBuf::default(); MAX_BUFS];
        for (k, vb) in var_bufs.iter_mut().enumerate() {
            let base = 48 + k * 16;
            *vb = VarBuf {
                tick_count: i32_at(b, base).expect("length checked"),
                buf_offset: non_negative(
                    "varBuf.bufOffset",
                    i32_at(b, base + 4).expect("length checked"),
                )?,
                tick_count_begin: i32_at(b, base + 8).expect("length checked"),
            };
        }
        let num_buf = non_negative("numBuf", i(8))?;
        if num_buf > MAX_BUFS {
            return Err(FormatError::Invalid {
                field: "numBuf",
                value: num_buf as i64,
            });
        }
        Ok(Self {
            ver: i(0),
            status: i(1),
            tick_rate: i(2),
            session_info_update: i(3),
            session_info_len: non_negative("sessionInfoLen", i(4))?,
            session_info_offset: non_negative("sessionInfoOffset", i(5))?,
            num_vars: non_negative("numVars", i(6))?,
            var_header_offset: non_negative("varHeaderOffset", i(7))?,
            num_buf,
            buf_len: non_negative("bufLen", i(9))?,
            cur_buf: b[44],
            var_bufs,
        })
    }

    /// True if the sim reports itself as connected.
    pub fn is_connected(&self) -> bool {
        self.status & STATUS_CONNECTED != 0
    }
}

/// `irsdk_diskSubHeader` (`.ibt` files only).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiskSubHeader {
    /// Wall-clock time (Unix seconds) when recording of this file started
    pub session_start_date: i64,
    /// Session time (s) of the first row
    pub session_start_time: f64,
    /// Session time (s) of the last row
    pub session_end_time: f64,
    /// Number of laps in the file
    pub session_lap_count: i32,
    /// Number of data rows
    pub session_record_count: i32,
}

impl DiskSubHeader {
    /// Parses the disk sub header from `b` (which starts at the sub header).
    pub fn parse(b: &[u8]) -> Result<Self, FormatError> {
        if b.len() < DISK_SUB_HEADER_LEN {
            return Err(FormatError::Truncated("disk sub header"));
        }
        Ok(Self {
            session_start_date: i64::from_le_bytes(b[0..8].try_into().expect("8 bytes")),
            session_start_time: f64_at(b, 8).expect("length checked"),
            session_end_time: f64_at(b, 16).expect("length checked"),
            session_lap_count: i32_at(b, 24).expect("length checked"),
            session_record_count: i32_at(b, 28).expect("length checked"),
        })
    }
}

/// `irsdk_VarType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarType {
    /// 1 byte
    Char,
    /// 1 byte
    Bool,
    /// 4 bytes, signed
    Int,
    /// 4 bytes, bit field
    BitField,
    /// 4 bytes, IEEE float
    Float,
    /// 8 bytes, IEEE double
    Double,
}

impl VarType {
    fn from_raw(v: i32) -> Option<Self> {
        Some(match v {
            0 => VarType::Char,
            1 => VarType::Bool,
            2 => VarType::Int,
            3 => VarType::BitField,
            4 => VarType::Float,
            5 => VarType::Double,
            _ => return None,
        })
    }

    /// Size in bytes (`irsdk_VarTypeBytes`).
    pub const fn size(self) -> usize {
        match self {
            VarType::Char | VarType::Bool => 1,
            VarType::Int | VarType::BitField | VarType::Float => 4,
            VarType::Double => 8,
        }
    }
}

/// `irsdk_varHeader`: describes one variable in a data row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarHeader {
    /// Value type
    pub var_type: VarType,
    /// Offset in the data row
    pub offset: usize,
    /// Number of entries (arrays have count > 1)
    pub count: usize,
    /// Name, e.g. `Speed`
    pub name: String,
    /// Description
    pub desc: String,
    /// Unit, e.g. `m/s`
    pub unit: String,
}

impl VarHeader {
    /// Reads entry `idx` from a data row as f64 (bools as 0/1).
    pub fn read(&self, row: &[u8], idx: usize) -> Option<f64> {
        if idx >= self.count {
            return None;
        }
        let off = self.offset + idx * self.var_type.size();
        let bytes = row.get(off..off + self.var_type.size())?;
        Some(match self.var_type {
            VarType::Char => f64::from(bytes[0] as i8),
            VarType::Bool => f64::from(u8::from(bytes[0] != 0)),
            VarType::Int => f64::from(i32::from_le_bytes(bytes.try_into().ok()?)),
            VarType::BitField => f64::from(u32::from_le_bytes(bytes.try_into().ok()?)),
            VarType::Float => f64::from(f32::from_le_bytes(bytes.try_into().ok()?)),
            VarType::Double => f64::from_le_bytes(bytes.try_into().ok()?),
        })
    }

    /// Zeroes all entries except `keep` (used to remove other cars' data
    /// from `CarIdx*` arrays).
    pub fn zero_except(&self, row: &mut [u8], keep: Option<usize>) {
        let size = self.var_type.size();
        for i in (0..self.count).filter(|&i| Some(i) != keep) {
            let off = self.offset + i * size;
            if let Some(slot) = row.get_mut(off..off + size) {
                slot.fill(0);
            }
        }
    }

    /// Byte length of this variable in a row.
    pub fn byte_len(&self) -> usize {
        self.count * self.var_type.size()
    }
}

/// The variable table of a session (all var headers, looked up by name).
#[derive(Debug, Clone, PartialEq)]
pub struct VarTable {
    vars: Vec<VarHeader>,
    by_name: HashMap<String, usize>,
    row_len: usize,
}

impl VarTable {
    /// Parses `num_vars` var headers from `b`. `row_len` is the data row
    /// length; variables that don't fit into a row are rejected.
    pub fn parse(b: &[u8], num_vars: usize, row_len: usize) -> Result<Self, FormatError> {
        let needed = num_vars
            .checked_mul(VAR_HEADER_LEN)
            .ok_or(FormatError::Truncated("var headers"))?;
        if b.len() < needed {
            return Err(FormatError::Truncated("var headers"));
        }
        let mut vars = Vec::with_capacity(num_vars);
        for k in 0..num_vars {
            let h = &b[k * VAR_HEADER_LEN..(k + 1) * VAR_HEADER_LEN];
            let raw_type = i32_at(h, 0).expect("length checked");
            let var_type = VarType::from_raw(raw_type).ok_or(FormatError::Invalid {
                field: "varHeader.type",
                value: i64::from(raw_type),
            })?;
            let var = VarHeader {
                var_type,
                offset: non_negative("varHeader.offset", i32_at(h, 4).expect("length checked"))?,
                count: non_negative("varHeader.count", i32_at(h, 8).expect("length checked"))?,
                name: c_string(&h[16..48]),
                desc: c_string(&h[48..112]),
                unit: c_string(&h[112..144]),
            };
            if var.offset + var.byte_len() > row_len {
                return Err(FormatError::Invalid {
                    field: "varHeader.offset",
                    value: var.offset as i64,
                });
            }
            vars.push(var);
        }
        let by_name = vars
            .iter()
            .enumerate()
            .map(|(i, v)| (v.name.clone(), i))
            .collect();
        Ok(Self {
            vars,
            by_name,
            row_len,
        })
    }

    /// Looks up a variable by name.
    pub fn get(&self, name: &str) -> Option<&VarHeader> {
        self.by_name.get(name).map(|&i| &self.vars[i])
    }

    /// All variables.
    pub fn vars(&self) -> &[VarHeader] {
        &self.vars
    }

    /// Length of one data row.
    pub fn row_len(&self) -> usize {
        self.row_len
    }

    /// Removes other cars' data from a live row: every `CarIdx*` array
    /// (one entry per car) is zeroed except the player's entry
    /// (docs/ARCHITECTURE.md §5.2). `.ibt` rows contain no such arrays.
    pub fn scrub_other_cars(&self, row: &mut [u8], player_idx: Option<usize>) {
        for var in self.vars.iter().filter(|v| v.name.starts_with("CarIdx")) {
            var.zero_except(row, player_idx);
        }
    }
}

/// Decodes a NUL-terminated, Latin-1 encoded fixed-size string.
pub(crate) fn c_string(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    latin1(&b[..end])
}

/// Decodes ISO-8859-1 bytes (iRacing's string encoding) to a String.
pub(crate) fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Builds synthetic telemetry in the iRacing format for tests.

    use super::*;

    /// A variable for [`build_var_table`]: (name, type, count, unit).
    pub type VarDef<'a> = (&'a str, VarType, usize, &'a str);

    /// Encodes var headers; returns (bytes, row length).
    pub fn build_var_table(defs: &[VarDef<'_>]) -> (Vec<u8>, usize) {
        let mut out = Vec::new();
        let mut offset = 0usize;
        for (name, ty, count, unit) in defs {
            let raw_type: i32 = match ty {
                VarType::Char => 0,
                VarType::Bool => 1,
                VarType::Int => 2,
                VarType::BitField => 3,
                VarType::Float => 4,
                VarType::Double => 5,
            };
            let mut h = vec![0u8; VAR_HEADER_LEN];
            h[0..4].copy_from_slice(&raw_type.to_le_bytes());
            h[4..8].copy_from_slice(&(offset as i32).to_le_bytes());
            h[8..12].copy_from_slice(&(*count as i32).to_le_bytes());
            h[16..16 + name.len()].copy_from_slice(name.as_bytes());
            h[112..112 + unit.len()].copy_from_slice(unit.as_bytes());
            out.extend_from_slice(&h);
            offset += ty.size() * count;
        }
        (out, offset)
    }

    /// Writes a value into a row.
    pub fn put(row: &mut [u8], var: &VarHeader, idx: usize, value: f64) {
        let off = var.offset + idx * var.var_type.size();
        match var.var_type {
            VarType::Char => row[off] = value as i8 as u8,
            VarType::Bool => row[off] = u8::from(value != 0.0),
            VarType::Int => row[off..off + 4].copy_from_slice(&(value as i32).to_le_bytes()),
            VarType::BitField => row[off..off + 4].copy_from_slice(&(value as u32).to_le_bytes()),
            VarType::Float => row[off..off + 4].copy_from_slice(&(value as f32).to_le_bytes()),
            VarType::Double => row[off..off + 8].copy_from_slice(&value.to_le_bytes()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    #[test]
    fn var_table_roundtrip_and_unaligned_reads() {
        let (bytes, row_len) = build_var_table(&[
            ("OnPitRoad", VarType::Bool, 1, ""),
            ("Speed", VarType::Float, 1, "m/s"),
            ("Lat", VarType::Double, 1, "deg"),
            ("CarIdxLap", VarType::Int, 4, ""),
        ]);
        let table = VarTable::parse(&bytes, 4, row_len).unwrap();
        let speed = table.get("Speed").unwrap();
        assert_eq!(speed.offset, 1, "packed without alignment");
        assert_eq!(speed.unit, "m/s");

        let mut row = vec![0u8; row_len];
        put(&mut row, speed, 0, 42.5);
        put(&mut row, table.get("Lat").unwrap(), 0, 52.027_115_321);
        put(&mut row, table.get("OnPitRoad").unwrap(), 0, 1.0);
        let lap = table.get("CarIdxLap").unwrap();
        for i in 0..4 {
            put(&mut row, lap, i, (i + 10) as f64);
        }
        assert_eq!(speed.read(&row, 0), Some(42.5));
        assert_eq!(
            table.get("Lat").unwrap().read(&row, 0),
            Some(52.027_115_321)
        );
        assert_eq!(table.get("OnPitRoad").unwrap().read(&row, 0), Some(1.0));
        assert_eq!(lap.read(&row, 3), Some(13.0));
        assert_eq!(lap.read(&row, 4), None, "out of range");

        lap.zero_except(&mut row, Some(2));
        let values: Vec<_> = (0..4).map(|i| lap.read(&row, i).unwrap()).collect();
        assert_eq!(values, [0.0, 0.0, 12.0, 0.0]);
        assert_eq!(speed.read(&row, 0), Some(42.5), "neighbours untouched");
    }

    #[test]
    fn scrub_keeps_only_the_player() {
        let (bytes, row_len) = build_var_table(&[
            ("Speed", VarType::Float, 1, ""),
            ("CarIdxLapDistPct", VarType::Float, 3, ""),
            ("CarIdxGear", VarType::Int, 3, ""),
        ]);
        let table = VarTable::parse(&bytes, 3, row_len).unwrap();
        let mut row = vec![0u8; row_len];
        put(&mut row, table.get("Speed").unwrap(), 0, 50.0);
        for i in 0..3 {
            put(&mut row, table.get("CarIdxLapDistPct").unwrap(), i, 0.5);
            put(&mut row, table.get("CarIdxGear").unwrap(), i, 4.0);
        }
        table.scrub_other_cars(&mut row, Some(1));
        let pct = table.get("CarIdxLapDistPct").unwrap();
        let gear = table.get("CarIdxGear").unwrap();
        assert_eq!(pct.read(&row, 0), Some(0.0));
        assert_eq!(pct.read(&row, 1), Some(0.5));
        assert_eq!(gear.read(&row, 2), Some(0.0));
        assert_eq!(table.get("Speed").unwrap().read(&row, 0), Some(50.0));
        table.scrub_other_cars(&mut row, None);
        assert_eq!(pct.read(&row, 1), Some(0.0), "unknown player: drop all");
    }

    #[test]
    fn rejects_bad_var_tables() {
        let (bytes, row_len) = build_var_table(&[("Speed", VarType::Float, 1, "")]);
        assert!(VarTable::parse(&bytes, 2, row_len).is_err(), "truncated");
        assert!(
            VarTable::parse(&bytes, 1, row_len - 1).is_err(),
            "var outside row"
        );
        let mut bad = bytes.clone();
        bad[0] = 9;
        assert!(VarTable::parse(&bad, 1, row_len).is_err(), "unknown type");
    }

    #[test]
    fn header_parsing() {
        let mut b = vec![0u8; HEADER_LEN];
        for (n, v) in [2i32, 1, 60, 3, 100, 4000, 287, 144, 1, 1098, 77]
            .iter()
            .enumerate()
        {
            b[n * 4..n * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        b[44] = 0;
        b[48..52].copy_from_slice(&5i32.to_le_bytes());
        b[52..56].copy_from_slice(&85_267i32.to_le_bytes());
        let h = Header::parse(&b).unwrap();
        assert_eq!(h.tick_rate, 60);
        assert_eq!(h.num_vars, 287);
        assert_eq!(h.buf_len, 1098);
        assert_eq!(h.var_bufs[0].buf_offset, 85_267);
        assert!(h.is_connected());
        assert!(Header::parse(&b[..100]).is_err());
    }
}
