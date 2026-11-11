// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Device login through the system browser (RFC 8252: loopback redirect +
//! PKCE). See `sy-server`'s `devices` module for the server side.
//!
//! ```text
//! app                         browser                         server
//!  │ listen 127.0.0.1:<port>
//!  │ open /device-login?… ───►  log in, approve ──────────────► code
//!  │ ◄──── GET /callback?code&state (redirect)
//!  │ POST /device-login/token {code, verifier} ───────────────► token
//! ```

use std::time::Duration;

use base64::Engine;
use sha2::{Digest, Sha256};
use sy_model::remote::NewDevice;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::client::{Client, SyncError};

fn random_token() -> String {
    let mut b = [0u8; 32];
    rand::fill(&mut b[..]);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                Ok(v) => {
                    out.push(v);
                    i += 3;
                    continue;
                }
                Err(_) => out.push(b'%'),
            },
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A device login in progress.
#[derive(Debug)]
pub struct DeviceLogin {
    client: Client,
    listener: TcpListener,
    verifier: String,
    state: String,
}

impl DeviceLogin {
    /// Starts listening for the browser's redirect. Returns the login and the
    /// address to open in the system browser.
    pub async fn start(client: Client, device_name: &str) -> Result<(Self, String), SyncError> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| SyncError::Login(format!("cannot listen for the login: {e}")))?;
        let port = listener
            .local_addr()
            .map_err(|e| SyncError::Login(e.to_string()))?
            .port();
        let verifier = random_token();
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()));
        let state = random_token();
        let redirect = format!("http://127.0.0.1:{port}/callback");
        let url = format!(
            "{}/device-login?redirect_uri={}&code_challenge={}&state={}&device_name={}",
            client.server_url(),
            url_encode(&redirect),
            challenge,
            state,
            url_encode(device_name)
        );
        Ok((
            Self {
                client,
                listener,
                verifier,
                state,
            },
            url,
        ))
    }

    /// Waits for the browser to come back (at most `timeout`), then fetches
    /// the device token.
    pub async fn finish(self, timeout: Duration) -> Result<NewDevice, SyncError> {
        tokio::time::timeout(timeout, self.wait())
            .await
            .map_err(|_| {
                SyncError::Login("the login in the browser was not finished in time".into())
            })?
    }

    async fn wait(self) -> Result<NewDevice, SyncError> {
        loop {
            let (mut stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|e| SyncError::Login(e.to_string()))?;
            let Some(target) = read_request_target(&mut stream).await else {
                continue;
            };
            let Some(query) = target.strip_prefix("/callback?") else {
                respond(&mut stream, 404, "Not found").await;
                continue;
            };
            let param = |name: &str| {
                query
                    .split('&')
                    .find_map(|p| p.strip_prefix(&format!("{name}=")))
                    .map(url_decode)
            };
            if param("state").as_deref() != Some(self.state.as_str()) {
                respond(
                    &mut stream,
                    400,
                    "This login link doesn't belong to this app. Please start again from Stintory.",
                )
                .await;
                return Err(SyncError::Login(
                    "the login answer didn't match (state)".into(),
                ));
            }
            let Some(code) = param("code") else {
                respond(&mut stream, 400, "The login was cancelled.").await;
                return Err(SyncError::Login("the login was cancelled".into()));
            };
            let result = self.client.exchange_code(&code, &self.verifier).await;
            match &result {
                Ok(_) => {
                    respond(
                        &mut stream,
                        200,
                        "Stintory is connected. You can close this tab and return to the app.",
                    )
                    .await
                }
                Err(e) => respond(&mut stream, 500, &format!("Connecting failed: {e}")).await,
            }
            return result;
        }
    }
}

/// Reads an HTTP request head and returns the target of a `GET`.
async fn read_request_target(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if n == 0 || buf.len() > 16 * 1024 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf);
    let mut parts = head.lines().next()?.split(' ');
    (parts.next()? == "GET").then(|| parts.next().map(str::to_owned))?
}

async fn respond(stream: &mut TcpStream, status: u16, message: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Internal Server Error",
    };
    let escaped = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Stintory</title>\
         <style>body{{font-family:system-ui,sans-serif;background:#09090b;color:#e4e4e7;\
         display:grid;place-items:center;height:100vh;margin:0}}</style></head>\
         <body><p>{escaped}</p></body></html>"
    );
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_coding() {
        assert_eq!(
            url_encode("http://127.0.0.1:5/cb"),
            "http%3A%2F%2F127.0.0.1%3A5%2Fcb"
        );
        assert_eq!(url_decode("a%20b+c%2F%"), "a b c/%");
        assert_eq!(url_decode(&url_encode("Rig PC ü")), "Rig PC ü");
    }

    #[tokio::test]
    async fn wrong_state_is_rejected() {
        let client = Client::new("http://127.0.0.1:9").unwrap();
        let (login, url) = DeviceLogin::start(client, "test").await.unwrap();
        assert!(
            url.starts_with(
                "http://127.0.0.1:9/device-login?redirect_uri=http%3A%2F%2F127.0.0.1%3A"
            )
        );
        let port = login.listener.local_addr().unwrap().port();
        let task = tokio::spawn(login.finish(Duration::from_secs(5)));
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(b"GET /callback?code=x&state=forged HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        let mut answer = String::new();
        s.read_to_string(&mut answer).await.unwrap();
        assert!(answer.starts_with("HTTP/1.1 400"));
        assert!(matches!(task.await.unwrap(), Err(SyncError::Login(_))));
    }
}
