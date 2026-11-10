// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! One sync run: push local changes, then pull the server's.
//!
//! Pushing first means local edits win over older server state; pulling
//! afterwards brings what other devices recorded. Both directions are
//! idempotent, so an interrupted run is simply repeated next time.

use std::collections::BTreeSet;

use rz_capture::pipeline::SharedStore;
use rz_model::BlobHash;
use rz_model::remote::SyncPush;
use rz_store::{PendingPush, SYNC_CURSOR, Store, StoreError};
use serde::{Deserialize, Serialize};

use crate::client::{Client, SyncError};

/// Laps per push request.
const PUSH_BATCH: usize = 500;
/// Changes per pull request.
const PULL_BATCH: u32 = 500;

/// What a sync run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    /// Sessions sent to the server
    pub pushed_sessions: u32,
    /// Laps sent to the server
    pub pushed_laps: u32,
    /// Deletions sent to the server
    pub pushed_deletions: u32,
    /// Telemetry files uploaded
    pub uploaded_files: u32,
    /// Sessions received
    pub pulled_sessions: u32,
    /// Laps received (new or changed)
    pub pulled_laps: u32,
    /// Sessions deleted because another device deleted them
    pub pulled_deletions: u32,
}

impl SyncReport {
    /// True if the local data changed (the UI should refresh).
    pub fn changed_local_data(&self) -> bool {
        self.pulled_sessions + self.pulled_laps + self.pulled_deletions > 0
    }
}

async fn with_store<T, F>(store: &SharedStore, f: F) -> Result<T, SyncError>
where
    F: FnOnce(&mut Store) -> Result<T, StoreError> + Send + 'static,
    T: Send + 'static,
{
    let store = store.clone();
    tokio::task::spawn_blocking(move || store.with(f))
        .await
        .map_err(|e| SyncError::Login(format!("internal error: {e}")))?
        .map_err(SyncError::from)
}

/// Pushes local changes, then pulls changes from the server.
pub async fn sync_once(client: &Client, store: &SharedStore) -> Result<SyncReport, SyncError> {
    let mut report = SyncReport::default();
    push(client, store, &mut report).await?;
    pull(client, store, &mut report).await?;
    tracing::info!(?report, "sync finished");
    Ok(report)
}

async fn push(
    client: &Client,
    store: &SharedStore,
    report: &mut SyncReport,
) -> Result<(), SyncError> {
    loop {
        let pending: PendingPush = with_store(store, |s| s.pending_push(PUSH_BATCH)).await?;
        if pending.is_empty() {
            return Ok(());
        }
        // telemetry files first: laps refer to them
        let hashes: BTreeSet<String> = pending
            .laps
            .iter()
            .map(|l| l.blob_hash.to_string())
            .collect();
        let mut missing = Vec::new();
        let hashes: Vec<String> = hashes.into_iter().collect();
        for chunk in hashes.chunks(1000) {
            missing.extend(client.missing_blobs(chunk.to_vec()).await?);
        }
        for hash in missing {
            let hash: BlobHash = hash
                .parse()
                .map_err(|_| SyncError::Login(format!("server sent an invalid hash {hash:?}")))?;
            let h = hash.clone();
            let bytes = with_store(store, move |s| s.lap_blob(&h)).await?;
            client.put_blob(&hash, bytes).await?;
            report.uploaded_files += 1;
        }
        // sessions (and deletions) before their laps
        let r = client
            .push(&SyncPush {
                sessions: pending.sessions.clone(),
                laps: Vec::new(),
                deleted_sessions: pending.deleted.clone(),
            })
            .await?;
        report.pushed_sessions += r.sessions;
        report.pushed_deletions += pending.deleted.len() as u32;
        for chunk in pending.laps.chunks(PUSH_BATCH) {
            let r = client
                .push(&SyncPush {
                    laps: chunk.to_vec(),
                    ..Default::default()
                })
                .await?;
            report.pushed_laps += r.laps;
        }
        let more = pending.laps.len() >= PUSH_BATCH;
        with_store(store, move |s| s.mark_pushed(&pending)).await?;
        if !more {
            return Ok(());
        }
    }
}

async fn pull(
    client: &Client,
    store: &SharedStore,
    report: &mut SyncReport,
) -> Result<(), SyncError> {
    let mut cursor: i64 = with_store(store, |s| s.setting(SYNC_CURSOR))
        .await?
        .unwrap_or(0);
    loop {
        let page = client.changes(cursor, PULL_BATCH).await?;
        let deleted = page.deleted_sessions.clone();
        let sessions = page.sessions.clone();
        report.pulled_deletions += deleted.len() as u32;
        report.pulled_sessions += sessions.len() as u32;
        with_store(store, move |s| {
            for id in deleted {
                s.apply_remote_deletion(id)?;
            }
            for session in &sessions {
                s.apply_remote_session(session)?;
            }
            Ok(())
        })
        .await?;
        for lap in page.laps {
            let probe = lap.clone();
            let needs = with_store(store, move |s| s.needs_telemetry(&probe)).await?;
            let bytes = if needs {
                Some(client.get_blob(&lap.blob_hash).await?)
            } else {
                None
            };
            let applied =
                with_store(store, move |s| s.apply_remote_lap(&lap, bytes.as_deref())).await?;
            if applied {
                report.pulled_laps += 1;
            }
        }
        cursor = page.cursor;
        with_store(store, move |s| s.set_setting(SYNC_CURSOR, &cursor)).await?;
        if !page.more {
            return Ok(());
        }
    }
}
