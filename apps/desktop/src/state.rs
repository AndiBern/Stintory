// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use sy_capture::pipeline::SharedStore;
use sy_model::api::{RecorderState, RecorderStatus};
use sy_store::{Store, StoreError};

use crate::recording::{F1Capture, LmuCapture};

/// Shared state of the desktop app, managed by Tauri.
pub struct AppState {
    /// The local store (locked per operation)
    pub store: SharedStore,
    /// Live recorder status, also sent as `recorder-status` event
    pub recorder: Arc<Mutex<RecorderStatus>>,
    /// Set on shutdown to stop background threads
    pub shutdown: Arc<AtomicBool>,
    /// F1 25 recording (UDP), shared with the settings commands
    pub f1: Arc<F1Capture>,
    /// Le Mans Ultimate recording, shared with the settings commands
    pub lmu: Arc<LmuCapture>,
}

impl AppState {
    /// Opens the local store in `data_dir`.
    pub fn open(data_dir: &Path) -> Result<Self, StoreError> {
        // F1 25 (UDP) can be recorded everywhere, iRacing on Windows
        let initial = RecorderState::Waiting;
        Ok(Self {
            store: SharedStore(Arc::new(Mutex::new(Store::open(data_dir)?))),
            recorder: Arc::new(Mutex::new(RecorderStatus::idle(initial))),
            shutdown: Arc::new(AtomicBool::new(false)),
            f1: Arc::default(),
            lmu: Arc::default(),
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
