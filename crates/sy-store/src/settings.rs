// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Key/value settings of the local client (stored as JSON values).

use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::store::Store;
use crate::{Result, timefmt};

/// Debug setting: keep raw spool files after they were processed.
pub const KEEP_RAW_RECORDINGS: &str = "debug.keep_raw_recordings";

/// Setting: driver name used for the last `.stintory` export (`Option<String>`).
pub const EXPORT_DRIVER_NAME: &str = "export.driver_name";

/// Setting: address of the sync server (`String`); unset = not connected.
pub const SYNC_SERVER_URL: &str = "sync.server_url";
/// Setting: position in the server's change feed (`i64`).
pub const SYNC_CURSOR: &str = "sync.cursor";
/// Setting: sync automatically (`bool`, default on).
pub const SYNC_AUTO: &str = "sync.auto";
/// Setting: recording F1 25 (`sy_model::api::F1CaptureSettings`).
pub const F1_CAPTURE: &str = "capture.f1";
/// Setting: recording Le Mans Ultimate (`sy_model::api::LmuCaptureSettings`).
pub const LMU_CAPTURE: &str = "capture.lmu";
/// Setting: the device token, only when the system keychain is unavailable
/// (`String`).
pub const SYNC_TOKEN_FALLBACK: &str = "sync.token";

impl Store {
    /// Reads a setting; `None` if it was never set.
    pub fn setting<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let value: Option<String> = self
            .conn()
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(value.map(|v| serde_json::from_str(&v)).transpose()?)
    }

    /// Writes a setting.
    pub fn set_setting<T: Serialize>(&mut self, key: &str, value: &T) -> Result<()> {
        self.conn().execute(
            "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, serde_json::to_string(value)?, timefmt::now()],
        )?;
        Ok(())
    }

    /// Removes a setting.
    pub fn remove_setting(&mut self, key: &str) -> Result<()> {
        self.conn()
            .execute("DELETE FROM settings WHERE key = ?1", [key])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        assert_eq!(store.setting::<bool>(KEEP_RAW_RECORDINGS).unwrap(), None);
        store.set_setting(KEEP_RAW_RECORDINGS, &true).unwrap();
        store.set_setting(KEEP_RAW_RECORDINGS, &false).unwrap();
        assert_eq!(
            store.setting::<bool>(KEEP_RAW_RECORDINGS).unwrap(),
            Some(false)
        );
    }
}
