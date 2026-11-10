// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! HTTP client for the server's device and sync API (`/api/v1`).

use std::sync::Once;
use std::time::Duration;

use reqwest::{RequestBuilder, StatusCode};
use rz_model::BlobHash;
use rz_model::remote::{
    BlobHashes, CurrentDevice, MissingBlobs, NewDevice, SyncChanges, SyncPush, SyncPushResult,
    TokenRequest,
};
use serde::de::DeserializeOwned;

/// Errors talking to the server.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// The server address is not usable
    #[error("invalid server address {0:?} (expected https://… or http://…)")]
    BadUrl(String),
    /// Network problem (offline, DNS, TLS, timeout)
    #[error("cannot reach the server: {0}")]
    Network(String),
    /// The device token was revoked (or the account disabled)
    #[error("this device was disconnected from the server")]
    Revoked,
    /// Any other answer than success
    #[error("{message}")]
    Server {
        /// HTTP status
        status: u16,
        /// Machine-readable reason, if any
        code: Option<String>,
        /// Message from the server
        message: String,
    },
    /// The device login failed or was cancelled
    #[error("{0}")]
    Login(String),
    /// Local store error
    #[error(transparent)]
    Store(#[from] rz_store::StoreError),
}

impl SyncError {
    fn network(e: reqwest::Error) -> Self {
        let mut msg = e.to_string();
        let mut source = std::error::Error::source(&e);
        while let Some(s) = source {
            msg = format!("{msg}: {s}");
            source = s.source();
        }
        Self::Network(msg)
    }
}

/// Uses `ring` for TLS: pure Rust plus small C parts, builds everywhere
/// without extra tools. Certificates are checked against the operating
/// system's trust store.
fn install_crypto_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Normalizes a server address: `https://host[:port][/path]`, no trailing
/// slash.
pub fn normalize_server_url(url: &str) -> Result<String, SyncError> {
    let url = url.trim().trim_end_matches('/');
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or_else(|| SyncError::BadUrl(url.to_owned()))?;
    if rest.is_empty() || rest.contains(char::is_whitespace) || rest.contains(['?', '#']) {
        return Err(SyncError::BadUrl(url.to_owned()));
    }
    Ok(url.to_owned())
}

/// A connection to a Rundenzeit server.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    server: String,
    token: Option<String>,
}

impl Client {
    /// A client for `server_url` (not logged in yet).
    pub fn new(server_url: &str) -> Result<Self, SyncError> {
        install_crypto_provider();
        let http = reqwest::Client::builder()
            .user_agent(concat!("Rundenzeit/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(SyncError::network)?;
        Ok(Self {
            http,
            server: normalize_server_url(server_url)?,
            token: None,
        })
    }

    /// The same client, authenticated with a device token.
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    /// The server's base address.
    pub fn server_url(&self) -> &str {
        &self.server
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/v1{path}", self.server)
    }

    fn auth(&self, req: RequestBuilder) -> RequestBuilder {
        match &self.token {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    async fn send(&self, req: RequestBuilder) -> Result<reqwest::Response, SyncError> {
        let res = self.auth(req).send().await.map_err(SyncError::network)?;
        if res.status().is_success() {
            return Ok(res);
        }
        let status = res.status();
        #[derive(serde::Deserialize)]
        struct Body {
            error: Option<String>,
            code: Option<String>,
        }
        let body: Option<Body> = res.json().await.ok();
        let code = body.as_ref().and_then(|b| b.code.clone());
        if status == StatusCode::UNAUTHORIZED && self.token.is_some() {
            return Err(SyncError::Revoked);
        }
        Err(SyncError::Server {
            status: status.as_u16(),
            message: body
                .and_then(|b| b.error)
                .unwrap_or_else(|| format!("the server answered {status}")),
            code,
        })
    }

    async fn json<T: DeserializeOwned>(&self, req: RequestBuilder) -> Result<T, SyncError> {
        self.send(req)
            .await?
            .json()
            .await
            .map_err(SyncError::network)
    }

    /// The device and user of the token.
    pub async fn current_device(&self) -> Result<CurrentDevice, SyncError> {
        self.json(self.http.get(self.url("/devices/current"))).await
    }

    /// Exchanges a device login code for a token (no token needed).
    pub async fn exchange_code(&self, code: &str, verifier: &str) -> Result<NewDevice, SyncError> {
        self.json(
            self.http
                .post(self.url("/device-login/token"))
                .json(&TokenRequest {
                    code: code.to_owned(),
                    code_verifier: verifier.to_owned(),
                }),
        )
        .await
    }

    /// Revokes this device's token on the server.
    pub async fn disconnect(&self) -> Result<(), SyncError> {
        self.send(self.http.delete(self.url("/devices/current")))
            .await
            .map(|_| ())
    }

    /// Which of the given telemetry files the server lacks.
    pub async fn missing_blobs(&self, hashes: Vec<String>) -> Result<Vec<String>, SyncError> {
        let r: MissingBlobs = self
            .json(
                self.http
                    .post(self.url("/sync/blobs/missing"))
                    .json(&BlobHashes { hashes }),
            )
            .await?;
        Ok(r.missing)
    }

    /// Uploads a telemetry file.
    pub async fn put_blob(&self, hash: &BlobHash, bytes: Vec<u8>) -> Result<(), SyncError> {
        self.send(
            self.http
                .put(self.url(&format!("/sync/blobs/{hash}")))
                .header("content-type", "application/vnd.apache.parquet")
                .body(bytes),
        )
        .await
        .map(|_| ())
    }

    /// Downloads a telemetry file.
    pub async fn get_blob(&self, hash: &BlobHash) -> Result<Vec<u8>, SyncError> {
        let res = self
            .send(self.http.get(self.url(&format!("/sync/blobs/{hash}"))))
            .await?;
        Ok(res.bytes().await.map_err(SyncError::network)?.to_vec())
    }

    /// Writes sessions/laps/deletions.
    pub async fn push(&self, push: &SyncPush) -> Result<SyncPushResult, SyncError> {
        self.json(self.http.post(self.url("/sync/push")).json(push))
            .await
    }

    /// One page of changes after `since`.
    pub async fn changes(&self, since: i64, limit: u32) -> Result<SyncChanges, SyncError> {
        self.json(
            self.http
                .get(self.url(&format!("/sync/changes?since={since}&limit={limit}"))),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_urls() {
        assert_eq!(
            normalize_server_url(" https://laps.example.com/ ").unwrap(),
            "https://laps.example.com"
        );
        assert_eq!(
            normalize_server_url("http://10.0.0.5:8080/rz").unwrap(),
            "http://10.0.0.5:8080/rz"
        );
        for bad in [
            "laps.example.com",
            "ftp://x",
            "https://",
            "https://a b",
            "https://x/?a",
        ] {
            assert!(normalize_server_url(bad).is_err(), "{bad}");
        }
    }
}
