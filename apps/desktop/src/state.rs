// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

use std::path::Path;
use std::sync::{Arc, Mutex};

use rz_store::{Store, StoreError};

/// Shared state of the desktop app, managed by Tauri.
pub struct AppState {
    store: Arc<Mutex<Store>>,
}

impl AppState {
    /// Opens the local store in `data_dir`.
    pub fn open(data_dir: &Path) -> Result<Self, StoreError> {
        Ok(Self {
            store: Arc::new(Mutex::new(Store::open(data_dir)?)),
        })
    }

    /// Runs `f` with exclusive access to the store on a blocking thread, so
    /// database and file I/O never block the UI thread.
    pub async fn with_store<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        F: FnOnce(&mut Store) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        let store = Arc::clone(&self.store);
        tauri::async_runtime::spawn_blocking(move || {
            // A panic while holding the lock poisons it; the store itself
            // stays consistent (transactions), so continue with the inner value.
            let mut guard = store.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut guard)
        })
        .await
        .expect("store task panicked")
    }
}
