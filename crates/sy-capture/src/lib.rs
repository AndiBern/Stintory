// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Game-independent capture pipeline (docs/ARCHITECTURE.md §5).
//!
//! ```text
//! game ─► RawSource ─RawRecord─► spool file
//!                        └──────► Decoder ─SourceEvent─► Recorder ─RecorderOutput─► store
//! ```
//!
//! The adapter interface ([`RawSource`], [`Decoder`], [`SourceEvent`], …)
//! lives in `sy_model::capture` so the permissively licensed game crates can
//! implement it.
//! - The [`Recorder`] turns these events into sessions and finished laps. It
//!   is pure logic (no I/O, no clock), so it is fully unit-testable and is
//!   shared by live capture, file import and crash recovery.

pub mod pipeline;
mod recorder;
pub mod spool;

pub use recorder::{RecordedLap, Recorder, RecorderConfig, RecorderOutput};
pub use sy_model::capture::{
    ChannelSpec, Decoder, Frame, RawRecord, RawSource, SessionMeta, SourceError, SourceEvent,
    ValueKind,
};
