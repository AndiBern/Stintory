// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! MCP endpoint (`POST /mcp`): lets an AI assistant (Claude Code, Claude
//! Desktop, …) read the user's sessions, laps and the corner analysis, to
//! explain where time is lost and how to improve. Read-only.
//!
//! Model Context Protocol over Streamable HTTP, stateless, for both protocol
//! eras:
//! - modern (2026-07-28): every request carries its protocol version and
//!   client capabilities in `params._meta`, mirrored in the
//!   `MCP-Protocol-Version`, `Mcp-Method` and `Mcp-Name` headers, which must
//!   match the body;
//! - legacy (2025-03-26 … 2025-11-25): an `initialize` handshake first. No
//!   session ids are handed out, so every request stands on its own.
//!
//! Every answer is one JSON object (no SSE streams); `GET` and `DELETE` get
//! 405. Authentication: `Authorization: Bearer` with a device token, normally
//! a read-only one created for this on the Account page.

mod tools;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use serde_json::{Value, json};

use crate::auth::{ClientIp, Session, bearer_token, check_restrictions};
use crate::ratelimit::PER_ADDRESS;
use crate::state::SharedState;

/// Protocol versions with per-request metadata, newest first.
const MODERN_VERSIONS: &[&str] = &["2026-07-28"];
/// Protocol versions with an `initialize` handshake, newest first.
const LEGACY_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];
/// Assumed for legacy requests without `MCP-Protocol-Version` (spec).
const LEGACY_DEFAULT: &str = "2025-03-26";

/// Tool calls allowed per user within [`crate::ratelimit::WINDOW`].
const CALLS_PER_WINDOW: u32 = 600;

const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

// JSON-RPC and MCP error codes
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const HEADER_MISMATCH: i64 = -32020;
const UNSUPPORTED_VERSION: i64 = -32022;

/// Guidance for the model, sent with the server's capabilities.
const INSTRUCTIONS: &str = "Stintory holds the user's sim racing telemetry (iRacing, F1, Le Mans \
Ultimate). Use it to find where the user loses time and how to drive faster.

Typical flow: list_tracks or list_sessions -> get_session (lap times, validity, conditions) -> \
best_laps for a track and car (the user's own laps and laps others shared with them) -> \
compare_laps for a lap against a faster reference -> lap_telemetry for a closer look at one \
corner.

compare_laps splits the lap into corners. Each corner's segment runs from before braking to the \
next corner and includes the straight after it, so the segment times add up to the lap time and \
timeLostS (positive = slower than the reference) tells where the gap comes from; entryLostS is \
the part up to the apex, exitLostS the part after it. Corners are numbered by Stintory in \
driving order, not by the track's official turn numbers. Distances are metres from the \
start/finish line, speeds km/h, pedals percent, times seconds.

Prefer valid laps as references. Ground advice in the numbers (braking point, apex speed, \
throttle pick-up, exit speed) and name the corners with the biggest gains first.";

fn server_info() -> Value {
    json!({ "name": "stintory", "title": "Stintory", "version": env!("CARGO_PKG_VERSION") })
}

/// `GET`/`DELETE /mcp`: no standalone streams and no sessions.
pub async fn not_allowed() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, HeaderValue::from_static("POST"))],
    )
        .into_response()
}

/// A JSON-RPC error answer.
fn rpc_error(status: StatusCode, id: Option<&Value>, code: i64, message: &str) -> Response {
    rpc_error_data(status, id, code, message, None)
}

fn rpc_error_data(
    status: StatusCode,
    id: Option<&Value>,
    code: i64,
    message: &str,
    data: Option<Value>,
) -> Response {
    let mut error = json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = data;
    }
    let mut body = json!({ "jsonrpc": "2.0", "error": error });
    if let Some(id) = id {
        body["id"] = id.clone();
    }
    (status, Json(body)).into_response()
}

fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static(r#"Bearer realm="Stintory""#),
        )],
        Json(json!({ "error": message })),
    )
        .into_response()
}

/// The configured public origin (`https://laps.example.com`).
fn own_origin(state: &SharedState) -> &str {
    let url = state.config.public_url.trim_end_matches('/');
    match url.find("://") {
        Some(i) => url[i + 3..].find('/').map_or(url, |j| &url[..i + 3 + j]),
        None => url,
    }
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// A header value, Base64-decoded if it uses the `=?base64?…?=` form.
fn decoded_header(headers: &HeaderMap, name: &str) -> Option<String> {
    let v = header_str(headers, name)?;
    match v
        .strip_prefix("=?base64?")
        .and_then(|r| r.strip_suffix("?="))
    {
        Some(b64) => base64::engine::general_purpose::STANDARD
            .decode(b64)
            .ok()
            .and_then(|b| String::from_utf8(b).ok()),
        None => Some(v.to_owned()),
    }
}

/// `POST /mcp`
pub async fn post(
    State(state): State<SharedState>,
    ip: ClientIp,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // DNS rebinding protection: browsers send Origin, MCP clients usually don't
    if let Some(origin) = header_str(&headers, "origin")
        && origin != own_origin(&state)
    {
        return rpc_error(
            StatusCode::FORBIDDEN,
            None,
            INVALID_REQUEST,
            "origin not allowed",
        );
    }

    let session = match authenticate(&state, &headers, &ip).await {
        Ok(s) => s,
        Err(response) => return *response,
    };

    let Ok(message) = serde_json::from_slice::<Value>(&body) else {
        return rpc_error(StatusCode::BAD_REQUEST, None, PARSE_ERROR, "invalid JSON");
    };
    let Some(message) = message.as_object() else {
        return rpc_error(
            StatusCode::BAD_REQUEST,
            None,
            INVALID_REQUEST,
            "expected one JSON-RPC message (batches are not supported)",
        );
    };
    let id = message.get("id").filter(|id| !id.is_null());
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        // a response from a legacy client: nothing to do
        if message.contains_key("result") || message.contains_key("error") {
            return StatusCode::ACCEPTED.into_response();
        }
        return rpc_error(
            StatusCode::BAD_REQUEST,
            id,
            INVALID_REQUEST,
            "missing method",
        );
    };
    let Some(id) = id else {
        // notifications (initialized, cancelled, …) need no answer
        return StatusCode::ACCEPTED.into_response();
    };
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));

    let meta_version = params
        .get("_meta")
        .and_then(|m| m.get(META_VERSION))
        .and_then(Value::as_str);
    if method == "initialize" {
        return legacy_initialize(id, &params);
    }
    match meta_version {
        Some(version) => modern(&state, &session, &headers, id, method, &params, version).await,
        None => legacy(&state, &session, &headers, id, method, &params).await,
    }
}

/// The user of the bearer token, or the error answer.
async fn authenticate(
    state: &SharedState,
    headers: &HeaderMap,
    ip: &ClientIp,
) -> Result<Session, Box<Response>> {
    let Some(token) = bearer_token(headers) else {
        return Err(Box::new(unauthorized(
            "a token is required: create one on the Account page (AI assistants)",
        )));
    };
    let key = format!("mcp-auth:{}", ip.0.as_deref().unwrap_or("?"));
    if !state.limiter.allowed(&key, PER_ADDRESS) {
        return Err(Box::new(
            (StatusCode::TOO_MANY_REQUESTS, "too many failed attempts").into_response(),
        ));
    }
    let session = match crate::devices::authenticate(state, token, ip).await {
        Ok(s) => s,
        Err(e) if e.status() == StatusCode::UNAUTHORIZED => {
            state.limiter.fail(&key);
            return Err(Box::new(unauthorized("unknown or revoked token")));
        }
        Err(e) => return Err(Box::new(e.into_response())),
    };
    check_restrictions(state, &session)
        .await
        .map_err(|e| Box::new(e.into_response()))?;
    Ok(session)
}

/// Legacy handshake: agrees on a protocol version and describes the server.
fn legacy_initialize(id: &Value, params: &Value) -> Response {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|v| LEGACY_VERSIONS.contains(v))
        .unwrap_or(LEGACY_VERSIONS[0]);
    let result = json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": server_info(),
        "instructions": INSTRUCTIONS,
    });
    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

/// A request of a modern client: the headers must mirror the body.
async fn modern(
    state: &SharedState,
    session: &Session,
    headers: &HeaderMap,
    id: &Value,
    method: &str,
    params: &Value,
    version: &str,
) -> Response {
    if !MODERN_VERSIONS.contains(&version) {
        let supported: Vec<&str> = MODERN_VERSIONS
            .iter()
            .chain(LEGACY_VERSIONS)
            .copied()
            .collect();
        return rpc_error_data(
            StatusCode::BAD_REQUEST,
            Some(id),
            UNSUPPORTED_VERSION,
            "Unsupported protocol version",
            Some(json!({ "supported": supported, "requested": version })),
        );
    }
    if !params
        .get("_meta")
        .and_then(|m| m.get(META_CAPABILITIES))
        .is_some_and(Value::is_object)
    {
        return rpc_error(
            StatusCode::BAD_REQUEST,
            Some(id),
            INVALID_PARAMS,
            "missing _meta.io.modelcontextprotocol/clientCapabilities",
        );
    }
    let mismatch = |what: &str| {
        rpc_error(
            StatusCode::BAD_REQUEST,
            Some(id),
            HEADER_MISMATCH,
            &format!("Header mismatch: {what}"),
        )
    };
    if header_str(headers, "mcp-protocol-version") != Some(version) {
        return mismatch("MCP-Protocol-Version is missing or differs from the body");
    }
    if header_str(headers, "mcp-method") != Some(method) {
        return mismatch("Mcp-Method is missing or differs from the body");
    }
    if method == "tools/call" {
        let name = params.get("name").and_then(Value::as_str);
        if decoded_header(headers, "mcp-name").as_deref() != name {
            return mismatch("Mcp-Name is missing or differs from the body");
        }
    }

    let result = match method {
        "server/discover" => {
            let supported: Vec<&str> = MODERN_VERSIONS
                .iter()
                .chain(LEGACY_VERSIONS)
                .copied()
                .collect();
            Ok(json!({
                "supportedVersions": supported,
                "capabilities": { "tools": {} },
                "instructions": INSTRUCTIONS,
            }))
        }
        _ => dispatch(state, session, method, params).await,
    };
    match result {
        Ok(mut result) => {
            result["resultType"] = json!("complete");
            result["_meta"] = json!({ META_SERVER_INFO: server_info() });
            Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
        }
        Err(Failure::NotFound) => rpc_error(
            StatusCode::NOT_FOUND,
            Some(id),
            METHOD_NOT_FOUND,
            &format!("Method not found: {method}"),
        ),
        Err(Failure::InvalidParams(m)) => {
            rpc_error(StatusCode::BAD_REQUEST, Some(id), INVALID_PARAMS, &m)
        }
        Err(Failure::RateLimited) => (
            StatusCode::TOO_MANY_REQUESTS,
            "too many tool calls, try again later",
        )
            .into_response(),
    }
}

/// A request of a legacy client (after `initialize`).
async fn legacy(
    state: &SharedState,
    session: &Session,
    headers: &HeaderMap,
    id: &Value,
    method: &str,
    params: &Value,
) -> Response {
    let version = header_str(headers, "mcp-protocol-version").unwrap_or(LEGACY_DEFAULT);
    if MODERN_VERSIONS.contains(&version) {
        return rpc_error(
            StatusCode::BAD_REQUEST,
            Some(id),
            INVALID_PARAMS,
            "missing _meta.io.modelcontextprotocol/protocolVersion",
        );
    }
    if !LEGACY_VERSIONS.contains(&version) {
        return rpc_error(
            StatusCode::BAD_REQUEST,
            Some(id),
            INVALID_REQUEST,
            &format!("unsupported protocol version {version}"),
        );
    }
    let response = match dispatch(state, session, method, params).await {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(Failure::NotFound) => {
            return rpc_error(
                StatusCode::OK,
                Some(id),
                METHOD_NOT_FOUND,
                &format!("Method not found: {method}"),
            );
        }
        Err(Failure::InvalidParams(m)) => {
            return rpc_error(StatusCode::OK, Some(id), INVALID_PARAMS, &m);
        }
        Err(Failure::RateLimited) => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                "too many tool calls, try again later",
            )
                .into_response();
        }
    };
    Json(response).into_response()
}

/// Why a request failed (protocol errors; a failing tool is a result).
enum Failure {
    NotFound,
    InvalidParams(String),
    RateLimited,
}

/// The methods both eras share.
async fn dispatch(
    state: &SharedState,
    session: &Session,
    method: &str,
    params: &Value,
) -> Result<Value, Failure> {
    match method {
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools::definitions() })),
        "tools/call" => {
            let key = format!("mcp-calls:{}", session.user.id);
            if !state.limiter.allowed(&key, CALLS_PER_WINDOW) {
                return Err(Failure::RateLimited);
            }
            state.limiter.fail(&key);
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Err(Failure::InvalidParams("missing tool name".into()));
            };
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let outcome = tools::call(state, session, name, &args)
                .await
                .ok_or_else(|| Failure::InvalidParams(format!("Unknown tool: {name}")))?;
            Ok(match outcome {
                Ok(text) => {
                    json!({ "content": [{ "type": "text", "text": text }], "isError": false })
                }
                Err(message) => {
                    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
                }
            })
        }
        _ => Err(Failure::NotFound),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_base64_headers() {
        let mut h = HeaderMap::new();
        h.insert(
            "mcp-name",
            HeaderValue::from_static("=?base64?bGlzdF9sYXBz?="),
        );
        assert_eq!(decoded_header(&h, "mcp-name").as_deref(), Some("list_laps"));
        h.insert("mcp-name", HeaderValue::from_static("list_sessions"));
        assert_eq!(
            decoded_header(&h, "mcp-name").as_deref(),
            Some("list_sessions")
        );
    }
}
