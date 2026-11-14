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
//! 405. Authentication: `Authorization: Bearer` with an OAuth access token
//! (see [`crate::oauth`]) or a device token, normally a read-only one created
//! for this on the Account page.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use serde_json::{Value, json};
use sy_mcp::{
    Failure, HEADER_MISMATCH, INVALID_PARAMS, INVALID_REQUEST, LEGACY_DEFAULT, LEGACY_VERSIONS,
    META_CAPABILITIES, META_VERSION, METHOD_NOT_FOUND, MODERN_VERSIONS, PARSE_ERROR,
    UNSUPPORTED_VERSION,
};
use sy_model::LapId;
use sy_model::api::{
    CompareData, CompareRequest, ReferenceLap, ReferenceScope, SessionDetail, SessionSummary,
};
use sy_model::remote::SharedTrack;
use uuid::Uuid;

use crate::auth::{Auth, ClientIp, Session, bearer_token, check_restrictions};
use crate::error::ApiError;
use crate::oauth;
use crate::ratelimit::PER_ADDRESS;
use crate::sharing::ReferenceQuery;
use crate::state::SharedState;

/// Tool calls allowed per user within [`crate::ratelimit::WINDOW`].
const CALLS_PER_WINDOW: u32 = 600;

/// The tools' data: the same handlers as the web UI, so access is checked
/// the same way (the user's own data and laps others shared with them).
struct ServerData<'a> {
    state: &'a SharedState,
    session: &'a Session,
}

/// An API error as a message for the model (details of server errors stay
/// in the log).
fn api_error(e: ApiError) -> String {
    if e.status().is_server_error() {
        tracing::error!(error = %e.message(), "MCP tool failed");
        "internal error".into()
    } else {
        e.message().to_owned()
    }
}

impl ServerData<'_> {
    fn auth(&self) -> Auth {
        Auth(self.session.clone())
    }
}

impl sy_mcp::Data for ServerData<'_> {
    async fn sessions(&self) -> Result<Vec<SessionSummary>, String> {
        crate::data::list_sessions(State(self.state.clone()), self.auth())
            .await
            .map(|Json(s)| s)
            .map_err(api_error)
    }

    async fn session(&self, id: Uuid) -> Result<SessionDetail, String> {
        crate::data::get_session(State(self.state.clone()), self.auth(), Path(id))
            .await
            .map(|Json(d)| d)
            .map_err(api_error)
    }

    async fn shared_tracks(&self) -> Result<Vec<SharedTrack>, String> {
        crate::sharing::shared_tracks(State(self.state.clone()), self.auth())
            .await
            .map(|Json(t)| t)
            .map_err(api_error)
    }

    async fn reference_laps(
        &self,
        track_key: &str,
        car_key: &str,
        scope: ReferenceScope,
    ) -> Result<Vec<ReferenceLap>, String> {
        let query = ReferenceQuery {
            track: track_key.to_owned(),
            car: car_key.to_owned(),
            scope,
        };
        crate::sharing::reference_laps(State(self.state.clone()), self.auth(), Query(query))
            .await
            .map(|Json(l)| l)
            .map_err(api_error)
    }

    async fn compare(&self, lap_ids: Vec<Uuid>, step_m: f32) -> Result<CompareData, String> {
        let request = CompareRequest {
            lap_ids: lap_ids.into_iter().map(LapId::from_uuid).collect(),
            step_m: Some(step_m),
        };
        crate::data::compare(State(self.state.clone()), self.auth(), Json(request))
            .await
            .map(|Json(d)| d)
            .map_err(api_error)
    }
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
    (status, Json(sy_mcp::error_message(id, code, message, data))).into_response()
}

/// 401 with the challenge that leads OAuth clients to the authorization
/// server (RFC 9728 §5.1).
fn unauthorized(state: &SharedState, message: &str, invalid_token: bool) -> Response {
    let error = if invalid_token {
        r#"error="invalid_token", "#
    } else {
        ""
    };
    let challenge = format!(
        r#"Bearer {error}resource_metadata="{}", scope="{}""#,
        oauth::resource_metadata_url(state),
        oauth::SCOPE
    );
    let value =
        HeaderValue::from_str(&challenge).unwrap_or_else(|_| HeaderValue::from_static("Bearer"));
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, value)],
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
            state,
            "authorization required: connect with OAuth or create a token on the Account page",
            false,
        )));
    };
    let key = format!("mcp-auth:{}", ip.0.as_deref().unwrap_or("?"));
    if !state.limiter.allowed(&key, PER_ADDRESS) {
        return Err(Box::new(
            (StatusCode::TOO_MANY_REQUESTS, "too many failed attempts").into_response(),
        ));
    }
    let rejected = || {
        state.limiter.fail(&key);
        Box::new(unauthorized(
            state,
            "unknown, expired or revoked token",
            true,
        ))
    };
    let session = if token.starts_with(oauth::ACCESS_PREFIX) {
        match oauth::authenticate_access(state, token, ip).await {
            Ok(Some(s)) => s,
            Ok(None) => return Err(rejected()),
            Err(e) => return Err(Box::new(e.into_response())),
        }
    } else {
        match crate::devices::authenticate(state, token, ip).await {
            Ok(s) => s,
            Err(e) if e.status() == StatusCode::UNAUTHORIZED => return Err(rejected()),
            Err(e) => return Err(Box::new(e.into_response())),
        }
    };
    check_restrictions(state, &session)
        .await
        .map_err(|e| Box::new(e.into_response()))?;
    Ok(session)
}

/// Legacy handshake: agrees on a protocol version and describes the server.
fn legacy_initialize(id: &Value, params: &Value) -> Response {
    let result = sy_mcp::initialize_result(params);
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
        let supported = sy_mcp::supported_versions();
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

    if let Some(limited) = rate_limited(state, session, method) {
        return limited;
    }
    match sy_mcp::dispatch(&ServerData { state, session }, method, params, true).await {
        Ok(result) => {
            let result = sy_mcp::modern_result(result);
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
    if let Some(limited) = rate_limited(state, session, method) {
        return limited;
    }
    let response =
        match sy_mcp::dispatch(&ServerData { state, session }, method, params, false).await {
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
        };
    Json(response).into_response()
}

/// Tool calls count against the user's limit; None while within it.
fn rate_limited(state: &SharedState, session: &Session, method: &str) -> Option<Response> {
    if method != "tools/call" {
        return None;
    }
    let key = format!("mcp-calls:{}", session.user.id);
    if !state.limiter.allowed(&key, CALLS_PER_WINDOW) {
        return Some(
            (
                StatusCode::TOO_MANY_REQUESTS,
                "too many tool calls, try again later",
            )
                .into_response(),
        );
    }
    state.limiter.fail(&key);
    None
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
