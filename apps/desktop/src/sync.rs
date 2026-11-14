// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sync with a Stintory server (docs/ARCHITECTURE.md §9.3, §9.5).
//!
//! - Connecting opens the server's device login in the system browser
//!   ([`sy_sync::DeviceLogin`]). The device token goes into the OS keychain
//!   (Windows Credential Manager, Secret Service on Linux); without a
//!   keychain it is kept in the app's database, and the UI says so.
//! - A background task syncs a few seconds after something changed
//!   ([`SyncManager::nudge`]), every few minutes, and on request.
//! - If the server revoked the token, the app disconnects and tells the user.
//! - Teams and sharing live on the server: the frontend's requests are passed
//!   on with the device token ([`SyncManager::server_request`]). Sessions
//!   others shared are kept locally as references while access lasts.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use sy_capture::pipeline::SharedStore;
use sy_model::SessionId;
use sy_model::api::{SyncState, SyncStatus};
use sy_store::{SYNC_AUTO, SYNC_CURSOR, SYNC_SERVER_URL, SYNC_TOKEN_FALLBACK, StoreError};
use sy_sync::{Client, DeviceLogin, SyncError, fetch_shared_session, sync_once};
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter};
use tauri_plugin_opener::OpenerExt;
use tokio::sync::Notify;

use crate::commands::EVENT_SESSIONS_CHANGED;

/// Event: sync status changed ([`SyncStatus`]).
pub const EVENT_SYNC_STATUS: &str = "sync-status";

/// Keychain service name of the device token (the account is the server URL).
const KEYCHAIN_SERVICE: &str = "de.stintory.desktop";
/// Settings: cached account and device name for display while offline.
const SYNC_USERNAME: &str = "sync.username";
const SYNC_DEVICE_NAME: &str = "sync.device_name";

/// How long the browser login may take.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Sync this long after the last change (collects a few laps).
const DEBOUNCE: Duration = Duration::from_secs(5);
/// Sync at least this often while connected.
const INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Owns the sync state of the app.
pub struct SyncManager {
    app: AppHandle,
    store: SharedStore,
    status: Mutex<SyncStatus>,
    wake: Notify,
    /// One sync at a time
    running: tokio::sync::Mutex<()>,
    /// The device login waiting for the browser
    login: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for SyncManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncManager").finish_non_exhaustive()
    }
}

/// Error of a sync command, as a message for the UI.
pub type SyncResult<T> = Result<T, String>;

fn message(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// This computer's name, for the device list on the server.
fn device_name() -> String {
    let name = std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    match name {
        Some(n) => format!("Stintory on {n}"),
        None => "Stintory desktop".into(),
    }
}

// ----- token storage ------------------------------------------------------

/// Stores the token; returns true if it went into the system keychain.
fn save_token(store: &SharedStore, server: &str, token: &str) -> Result<bool, StoreError> {
    let in_keychain = keyring::Entry::new(KEYCHAIN_SERVICE, server)
        .and_then(|e| e.set_password(token))
        .map_err(|e| tracing::warn!(error = %e, "system keychain not available, keeping the device token in the app data"))
        .is_ok();
    store.with(|s| {
        if in_keychain {
            s.remove_setting(SYNC_TOKEN_FALLBACK)
        } else {
            s.set_setting(SYNC_TOKEN_FALLBACK, &token)
        }
    })?;
    Ok(in_keychain)
}

/// The stored token and whether it came from the keychain.
fn load_token(store: &SharedStore, server: &str) -> Result<Option<(String, bool)>, StoreError> {
    if let Ok(token) = keyring::Entry::new(KEYCHAIN_SERVICE, server).and_then(|e| e.get_password())
    {
        return Ok(Some((token, true)));
    }
    Ok(store
        .with(|s| s.setting::<String>(SYNC_TOKEN_FALLBACK))?
        .map(|t| (t, false)))
}

fn delete_token(store: &SharedStore, server: &str) -> Result<(), StoreError> {
    if let Ok(entry) = keyring::Entry::new(KEYCHAIN_SERVICE, server) {
        let _ = entry.delete_credential();
    }
    store.with(|s| s.remove_setting(SYNC_TOKEN_FALLBACK))
}

/// Runs blocking work (keychain, database) off the async runtime.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .expect("blocking task panicked")
}

// ----- manager ------------------------------------------------------------

impl SyncManager {
    /// Loads the saved connection and starts the background task.
    pub fn start(app: AppHandle, store: SharedStore) -> Arc<Self> {
        let manager = Arc::new(Self {
            app,
            store,
            status: Mutex::new(SyncStatus::disconnected()),
            wake: Notify::new(),
            running: tokio::sync::Mutex::new(()),
            login: Mutex::new(None),
        });
        let m = manager.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = m.load().await {
                tracing::error!(error = %e, "could not load the sync settings");
            }
            m.background().await;
        });
        manager
    }

    async fn load(&self) -> Result<(), StoreError> {
        let store = self.store.clone();
        let (url, user, device, auto) = blocking(move || {
            store.with(|s| {
                Ok::<_, StoreError>((
                    s.setting::<String>(SYNC_SERVER_URL)?,
                    s.setting::<String>(SYNC_USERNAME)?,
                    s.setting::<String>(SYNC_DEVICE_NAME)?,
                    s.setting::<bool>(SYNC_AUTO)?.unwrap_or(true),
                ))
            })
        })
        .await?;
        let in_keychain = match &url {
            Some(url) => {
                let (store, url) = (self.store.clone(), url.clone());
                blocking(move || load_token(&store, &url))
                    .await?
                    .map(|(_, k)| k)
            }
            None => None,
        };
        self.update(|s| {
            s.auto_sync = auto;
            if let (Some(url), Some(in_keychain)) = (url, in_keychain) {
                s.state = SyncState::Idle;
                s.server_url = Some(url);
                s.username = user;
                s.device_name = device;
                s.token_in_keychain = in_keychain;
            }
        });
        Ok(())
    }

    /// Current status.
    pub fn status(&self) -> SyncStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn update(&self, f: impl FnOnce(&mut SyncStatus)) -> SyncStatus {
        let snapshot = {
            let mut s = self.status.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut s);
            s.clone()
        };
        let _ = self.app.emit(EVENT_SYNC_STATUS, snapshot.clone());
        snapshot
    }

    /// Something changed locally: sync soon (if connected and automatic).
    pub fn nudge(&self) {
        let s = self.status();
        if s.auto_sync && matches!(s.state, SyncState::Idle | SyncState::Syncing) {
            self.wake.notify_one();
        }
    }

    async fn background(self: Arc<Self>) {
        // first sync shortly after start
        tokio::time::sleep(Duration::from_secs(3)).await;
        loop {
            let s = self.status();
            if s.auto_sync && s.state == SyncState::Idle {
                let _ = self.run().await;
            }
            tokio::select! {
                _ = self.wake.notified() => tokio::time::sleep(DEBOUNCE).await,
                _ = tokio::time::sleep(INTERVAL) => {}
            }
        }
    }

    /// A client for the connected server, logged in as this device.
    async fn client(&self) -> SyncResult<Client> {
        let Some(url) = self.status().server_url else {
            return Err("not connected to a server".into());
        };
        let (store, u) = (self.store.clone(), url.clone());
        let Some((token, _)) = blocking(move || load_token(&store, &u))
            .await
            .map_err(message)?
        else {
            self.forget_connection(
                &url,
                Some("The device token is missing. Please connect again.".into()),
            )
            .await;
            return Err("not connected to a server".into());
        };
        Ok(Client::new(&url).map_err(message)?.with_token(token))
    }

    /// Tells the user when the server doesn't know this device anymore.
    async fn check_revoked(&self, e: SyncError) -> String {
        if !matches!(e, SyncError::Revoked) {
            return e.to_string();
        }
        let msg = "This device was disconnected from the server (or your account was disabled). Connect again to sync.".to_owned();
        if let Some(url) = self.status().server_url {
            self.forget_connection(&url, Some(msg.clone())).await;
        }
        msg
    }

    /// A request of the server's teams and sharing API as this device's
    /// user (see `commands::server_request`).
    pub async fn server_request(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> SyncResult<serde_json::Value> {
        let client = self.client().await?;
        match client.request(method, path, body).await {
            Ok(v) => Ok(v),
            Err(e) => Err(self.check_revoked(e).await),
        }
    }

    /// Laps others shared with the user for a track and car, from the
    /// server. None when not connected.
    pub async fn shared_reference_laps(
        &self,
        track_key: &str,
        car_key: &str,
    ) -> SyncResult<Option<Vec<sy_model::api::ReferenceLap>>> {
        if self.status().server_url.is_none() {
            return Ok(None);
        }
        let client = self.client().await?;
        match client.shared_reference_laps(track_key, car_key).await {
            Ok(v) => Ok(Some(v)),
            Err(e) => Err(self.check_revoked(e).await),
        }
    }

    /// Keeps a session someone shared as a local reference (or refreshes
    /// the copy); returns the id of the local session.
    pub async fn open_shared(&self, owner: uuid::Uuid, id: SessionId) -> SyncResult<SessionId> {
        let client = self.client().await?;
        match fetch_shared_session(&client, &self.store, owner, id).await {
            Ok(local) => {
                let _ = self.app.emit(EVENT_SESSIONS_CHANGED, ());
                Ok(local)
            }
            Err(e) => Err(self.check_revoked(e).await),
        }
    }

    /// Runs one sync now (waits for a running one to finish first).
    pub async fn run(&self) -> SyncResult<SyncStatus> {
        let _guard = self.running.lock().await;
        let Some(url) = self.status().server_url else {
            return Err("not connected to a server".into());
        };
        let client = self.client().await?;
        self.update(|s| s.state = SyncState::Syncing);
        match sync_once(&client, &self.store).await {
            Ok(report) => {
                if report.changed_local_data() {
                    let _ = self.app.emit(EVENT_SESSIONS_CHANGED, ());
                }
                Ok(self.update(|s| {
                    s.state = SyncState::Idle;
                    s.last_sync_at = Some(time::OffsetDateTime::now_utc());
                    s.last_pushed = report.pushed_sessions + report.pushed_laps;
                    s.last_pulled = report.pulled_sessions + report.pulled_laps;
                    s.last_error = None;
                }))
            }
            Err(SyncError::Revoked) => {
                let msg = "This device was disconnected from the server (or your account was disabled). Connect again to sync.".to_owned();
                self.forget_connection(&url, Some(msg.clone())).await;
                Err(msg)
            }
            Err(e) => {
                tracing::warn!(error = %e, "sync failed");
                let msg = e.to_string();
                self.update(|s| {
                    s.state = SyncState::Idle;
                    s.last_error = Some(msg.clone());
                });
                Err(msg)
            }
        }
    }

    /// Connects through the browser: opens the server's login page and waits
    /// until the user approved this device (or cancelled).
    pub async fn connect(self: &Arc<Self>, server_url: &str) -> SyncResult<SyncStatus> {
        let client = Client::new(server_url).map_err(message)?;
        let url = client.server_url().to_owned();
        let name = device_name();
        let (login, browser_url) = DeviceLogin::start(client, &name).await.map_err(message)?;
        let previous = self.status();
        self.update(|s| {
            s.state = SyncState::Connecting;
            s.last_error = None;
        });
        if let Err(e) = self.app.opener().open_url(&browser_url, None::<&str>) {
            self.update(|s| *s = previous.clone());
            return Err(format!("cannot open the browser: {e}"));
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = tauri::async_runtime::spawn(async move {
            let _ = tx.send(login.finish(LOGIN_TIMEOUT).await);
        });
        *self.login.lock().unwrap_or_else(|e| e.into_inner()) = Some(task);
        let result = rx.await;
        self.login.lock().unwrap_or_else(|e| e.into_inner()).take();
        match result {
            Ok(Ok(device)) => {
                self.save_connection(
                    &url,
                    &device.token,
                    &device.user.username,
                    &device.device.name,
                )
                .await
            }
            Ok(Err(e)) => {
                self.update(|s| {
                    *s = previous.clone();
                    s.last_error = Some(e.to_string());
                });
                Err(e.to_string())
            }
            Err(_) => {
                // cancelled
                self.update(|s| *s = previous.clone());
                Err("connecting was cancelled".into())
            }
        }
    }

    /// Stops waiting for the browser login.
    pub fn cancel_connect(&self) {
        if let Some(task) = self.login.lock().unwrap_or_else(|e| e.into_inner()).take() {
            task.abort();
        }
    }

    /// Connects with a device token created on the server's account page.
    pub async fn connect_with_token(
        &self,
        server_url: &str,
        token: &str,
    ) -> SyncResult<SyncStatus> {
        let token = token.trim();
        let client = Client::new(server_url).map_err(message)?.with_token(token);
        let current = client.current_device().await.map_err(|e| match e {
            SyncError::Revoked => "the server doesn't accept this token".to_owned(),
            e => e.to_string(),
        })?;
        let name = current.device.map(|d| d.name).unwrap_or_else(device_name);
        let url = client.server_url().to_owned();
        self.save_connection(&url, token, &current.user.username, &name)
            .await
    }

    async fn save_connection(
        &self,
        url: &str,
        token: &str,
        username: &str,
        device: &str,
    ) -> SyncResult<SyncStatus> {
        let previous = self.status().server_url;
        let (store, url2, token2) = (self.store.clone(), url.to_owned(), token.to_owned());
        let (user2, device2) = (username.to_owned(), device.to_owned());
        let in_keychain = blocking(move || -> Result<bool, StoreError> {
            if let Some(old) = previous.filter(|old| *old != url2) {
                delete_token(&store, &old)?;
            }
            // another server (or a new connection) starts from scratch
            store.with(|s| {
                if s.setting::<String>(SYNC_SERVER_URL)?.as_deref() != Some(url2.as_str()) {
                    s.reset_sync()?;
                    s.remove_all_foreign()?;
                    s.remove_setting(SYNC_CURSOR)?;
                }
                s.set_setting(SYNC_SERVER_URL, &url2)?;
                s.set_setting(SYNC_USERNAME, &user2)?;
                s.set_setting(SYNC_DEVICE_NAME, &device2)
            })?;
            save_token(&store, &url2, &token2)
        })
        .await
        .map_err(message)?;
        let status = self.update(|s| {
            s.state = SyncState::Idle;
            s.server_url = Some(url.to_owned());
            s.username = Some(username.to_owned());
            s.device_name = Some(device.to_owned());
            s.token_in_keychain = in_keychain;
            s.last_error = None;
            s.last_sync_at = None;
        });
        self.wake.notify_one();
        Ok(status)
    }

    /// Disconnects: revokes the token on the server (best effort) and
    /// forgets the connection. Local data stays.
    pub async fn disconnect(&self) -> SyncResult<SyncStatus> {
        let _guard = self.running.lock().await;
        let Some(url) = self.status().server_url else {
            return Ok(self.status());
        };
        let (store, u) = (self.store.clone(), url.clone());
        if let Ok(Some((token, _))) = blocking(move || load_token(&store, &u)).await
            && let Ok(client) = Client::new(&url)
        {
            let client = client.with_token(token);
            if let Err(e) = tokio::time::timeout(Duration::from_secs(10), client.disconnect()).await
            {
                tracing::warn!(error = %e, "could not reach the server to revoke the device token");
            }
        }
        self.forget_connection(&url, None).await;
        Ok(self.status())
    }

    async fn forget_connection(&self, url: &str, error: Option<String>) {
        let (store, url) = (self.store.clone(), url.to_owned());
        let result = blocking(move || -> Result<(), StoreError> {
            delete_token(&store, &url)?;
            store.with(|s| {
                s.reset_sync()?;
                // laps of others can't be checked for access anymore
                s.remove_all_foreign()?;
                for key in [
                    SYNC_SERVER_URL,
                    SYNC_CURSOR,
                    SYNC_USERNAME,
                    SYNC_DEVICE_NAME,
                ] {
                    s.remove_setting(key)?;
                }
                Ok(())
            })
        })
        .await;
        if let Err(e) = result {
            tracing::error!(error = %e, "could not remove the sync settings");
        }
        let _ = self.app.emit(EVENT_SESSIONS_CHANGED, ());
        self.update(|s| {
            let auto = s.auto_sync;
            *s = SyncStatus::disconnected();
            s.auto_sync = auto;
            s.last_error = error;
        });
    }

    /// Turns automatic sync on or off.
    pub async fn set_auto(&self, auto: bool) -> SyncResult<SyncStatus> {
        let store = self.store.clone();
        blocking(move || store.with(|s| s.set_setting(SYNC_AUTO, &auto)))
            .await
            .map_err(message)?;
        let status = self.update(|s| s.auto_sync = auto);
        if auto {
            self.wake.notify_one();
        }
        Ok(status)
    }
}
