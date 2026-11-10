// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared helpers for the real-data tests.

use std::time::Duration;

use rz_model::GameId;
use rz_model::capture::{RawRecord, RawSource, SourceError};
use rz_source_iracing::IbtSource;
use rz_source_iracing::decoder::{KIND_SESSION_INFO, KIND_VAR_TABLE};
use rz_source_iracing::format::VAR_HEADER_LEN;

/// An `.ibt` replayed like live telemetry: the live variable table has no
/// GPS (`Lat`/`Lon`/`Alt` exist only in files). Optionally shifts the
/// session's wall-clock start, as a live recording may see it differently.
pub struct LiveLike {
    inner: IbtSource,
    shift_start_s: f64,
}

impl LiveLike {
    pub fn new(inner: IbtSource) -> Self {
        Self {
            inner,
            shift_start_s: 0.0,
        }
    }

    #[allow(dead_code)] // not every test binary uses it
    pub fn with_start_shifted(mut self, seconds: f64) -> Self {
        self.shift_start_s = seconds;
        self
    }
}

impl RawSource for LiveLike {
    fn game(&self) -> GameId {
        GameId::Iracing
    }

    fn next_record(&mut self, timeout: Duration) -> Result<Option<RawRecord>, SourceError> {
        let mut rec = self.inner.next_record(timeout)?;
        match rec.as_mut() {
            Some(r) if r.kind == KIND_VAR_TABLE => {
                for h in r.payload[12..].as_chunks_mut::<VAR_HEADER_LEN>().0 {
                    let name = &mut h[16..48];
                    if [&b"Lat\0"[..], b"Lon\0", b"Alt\0"]
                        .iter()
                        .any(|n| name.starts_with(n))
                    {
                        name[0] = b'_'; // hide it
                    }
                }
            }
            Some(r) if r.kind == KIND_SESSION_INFO && self.shift_start_s != 0.0 => {
                let mut info: serde_json::Value = serde_json::from_slice(&r.payload).unwrap();
                let epoch = info["session_epoch_unix"].as_f64().unwrap();
                info["session_epoch_unix"] = (epoch + self.shift_start_s).into();
                r.payload = serde_json::to_vec(&info).unwrap();
            }
            _ => {}
        }
        Ok(rec)
    }
}
