// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use sy_capture::pipeline::SharedStore;
use sy_model::api::{RecorderState, RecorderStatus};
use sy_store::{Store, StoreError};

/// Shared state of the desktop app, managed by Tauri.
pub struct AppState {
    /// The local store (locked per operation)
    pub store: SharedStore,
    /// Live recorder status, also sent as `recorder-status` event
    pub recorder: Arc<Mutex<RecorderStatus>>,
    /// Set on shutdown to stop background threads
    pub shutdown: Arc<AtomicBool>,
}

impl AppState {
    /// Opens the local store in `data_dir`.
    pub fn open(data_dir: &Path) -> Result<Self, StoreError> {
        let initial = if cfg!(windows) {
            RecorderState::Waiting
        } else {
            RecorderState::Unsupported
        };
        Ok(Self {
            store: SharedStore(Arc::new(Mutex::new(Store::open(data_dir)?))),
            recorder: Arc::new(Mutex::new(RecorderStatus::idle(initial))),
            shutdown: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Runs `f` with the store on a blocking thread, so database and file
    /// I/O never block the UI thread.
    pub async fn with_store<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        F: FnOnce(&mut Store) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        let store = self.store.clone();
        tauri::async_runtime::spawn_blocking(move || store.with(f))
            .await
            .expect("store task panicked")
    }
}
