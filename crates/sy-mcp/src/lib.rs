// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Model Context Protocol (MCP) for Stintory: lets an AI assistant read
//! sessions, laps and the corner analysis to explain where time is lost.
//! Read-only.
//!
//! Shared by sy-server (`POST /mcp`, Streamable HTTP, with its own
//! authentication and header checks) and the desktop app (`stintory mcp`,
//! stdio, local data). Both eras of the protocol are served:
//! - modern (2026-07-28): every request carries its protocol version and
//!   client capabilities in `params._meta`; no handshake;
//! - legacy (2025-03-26 … 2025-11-25): an `initialize` handshake first.
//!
//! The data comes from a [`Data`] source.

mod tools;

use std::future::Future;

use serde_json::{Value, json};
use sy_model::api::{CompareData, ReferenceLap, ReferenceScope, SessionDetail, SessionSummary};
use sy_model::remote::SharedTrack;
use uuid::Uuid;

pub use tools::definitions as tool_definitions;

/// Protocol versions with per-request metadata, newest first.
pub const MODERN_VERSIONS: &[&str] = &["2026-07-28"];
/// Protocol versions with an `initialize` handshake, newest first.
pub const LEGACY_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];
/// Assumed for legacy HTTP requests without `MCP-Protocol-Version` (spec).
pub const LEGACY_DEFAULT: &str = "2025-03-26";

/// `_meta` key of the request's protocol version.
pub const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
/// `_meta` key of the client capabilities.
pub const META_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
/// `_meta` key of the server identity in results.
pub const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

/// JSON-RPC: invalid JSON.
pub const PARSE_ERROR: i64 = -32700;
/// JSON-RPC: not a valid request.
pub const INVALID_REQUEST: i64 = -32600;
/// JSON-RPC: unknown method.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC: invalid parameters.
pub const INVALID_PARAMS: i64 = -32602;
/// MCP: HTTP headers don't match the body.
pub const HEADER_MISMATCH: i64 = -32020;
/// MCP: protocol version not supported.
pub const UNSUPPORTED_VERSION: i64 = -32022;

/// Guidance for the model, sent with the server's capabilities.
pub const INSTRUCTIONS: &str = "Stintory holds the user's sim racing telemetry (iRacing, F1, Le \
Mans Ultimate). Use it to find where the user loses time and how to drive faster.

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

compare_laps also says whether both laps were driven with the same car setup. If the setups \
differ, mention it: part of the gap may come from the setup rather than the driving.

Prefer valid laps as references. Ground advice in the numbers (braking point, apex speed, \
throttle pick-up, exit speed) and name the corners with the biggest gains first.";

/// Where the tools get their data. Errors are messages for the model.
pub trait Data: Sync {
    /// The user's sessions, newest first.
    fn sessions(&self) -> impl Future<Output = Result<Vec<SessionSummary>, String>> + Send;
    /// One of the user's sessions with its laps.
    fn session(&self, id: Uuid) -> impl Future<Output = Result<SessionDetail, String>> + Send;
    /// Tracks and cars with laps others share with the user (empty where
    /// there is no sharing).
    fn shared_tracks(&self) -> impl Future<Output = Result<Vec<SharedTrack>, String>> + Send;
    /// The fastest clean laps for a track and car.
    fn reference_laps(
        &self,
        track_key: &str,
        car_key: &str,
        scope: ReferenceScope,
    ) -> impl Future<Output = Result<Vec<ReferenceLap>, String>> + Send;
    /// Laps on a common distance grid with the corner analysis (first lap =
    /// reference).
    fn compare(
        &self,
        lap_ids: Vec<Uuid>,
        step_m: f32,
    ) -> impl Future<Output = Result<CompareData, String>> + Send;
}

/// Name and version of this implementation.
pub fn server_info() -> Value {
    json!({ "name": "stintory", "title": "Stintory", "version": env!("CARGO_PKG_VERSION") })
}

/// Why a request failed (protocol errors; a failing tool is a result).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Unknown method
    NotFound,
    /// Bad parameters or an unknown tool
    InvalidParams(String),
}

/// The result of a legacy `initialize`: agrees on a protocol version.
pub fn initialize_result(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|v| LEGACY_VERSIONS.contains(v))
        .unwrap_or(LEGACY_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": server_info(),
        "instructions": INSTRUCTIONS,
    })
}

/// All supported protocol versions, modern first.
pub fn supported_versions() -> Vec<&'static str> {
    MODERN_VERSIONS
        .iter()
        .chain(LEGACY_VERSIONS)
        .copied()
        .collect()
}

/// The result of a modern `server/discover`.
fn discover_result() -> Value {
    json!({
        "supportedVersions": supported_versions(),
        "capabilities": { "tools": {} },
        "instructions": INSTRUCTIONS,
    })
}

/// The methods every request era shares: `ping`, `tools/list`, `tools/call`
/// (and `server/discover` for modern requests).
pub async fn dispatch<D: Data>(
    data: &D,
    method: &str,
    params: &Value,
    modern: bool,
) -> Result<Value, Failure> {
    match method {
        "ping" => Ok(json!({})),
        "server/discover" if modern => Ok(discover_result()),
        "tools/list" => Ok(json!({ "tools": tools::definitions() })),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Err(Failure::InvalidParams("missing tool name".into()));
            };
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let outcome = tools::call(data, name, &args)
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

/// A modern result: with `resultType` and the server's identity.
pub fn modern_result(mut result: Value) -> Value {
    result["resultType"] = json!("complete");
    result["_meta"] = json!({ META_SERVER_INFO: server_info() });
    result
}

/// A JSON-RPC error message.
pub fn error_message(id: Option<&Value>, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = data;
    }
    let mut body = json!({ "jsonrpc": "2.0", "error": error });
    if let Some(id) = id {
        body["id"] = id.clone();
    }
    body
}

/// Handles one message of a stdio client (no headers to check). Returns the
/// answer for a request, None for notifications and responses.
pub async fn handle_message<D: Data>(data: &D, message: &Value) -> Option<Value> {
    let Some(message) = message.as_object() else {
        return Some(error_message(
            None,
            INVALID_REQUEST,
            "expected one JSON-RPC message (batches are not supported)",
            None,
        ));
    };
    let id = message.get("id").filter(|id| !id.is_null());
    let method = message.get("method").and_then(Value::as_str);
    let (Some(id), Some(method)) = (id, method) else {
        // notifications and responses need no answer
        return match (id, method) {
            (Some(id), None)
                if !message.contains_key("result") && !message.contains_key("error") =>
            {
                Some(error_message(
                    Some(id),
                    INVALID_REQUEST,
                    "missing method",
                    None,
                ))
            }
            _ => None,
        };
    };
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
    let respond = |result: Value| json!({ "jsonrpc": "2.0", "id": id, "result": result });
    if method == "initialize" {
        return Some(respond(initialize_result(&params)));
    }
    let meta = params.get("_meta");
    let version = meta
        .and_then(|m| m.get(META_VERSION))
        .and_then(Value::as_str);
    let modern = version.is_some();
    if let Some(version) = version {
        if !MODERN_VERSIONS.contains(&version) {
            return Some(error_message(
                Some(id),
                UNSUPPORTED_VERSION,
                "Unsupported protocol version",
                Some(json!({ "supported": supported_versions(), "requested": version })),
            ));
        }
        if !meta
            .and_then(|m| m.get(META_CAPABILITIES))
            .is_some_and(Value::is_object)
        {
            return Some(error_message(
                Some(id),
                INVALID_PARAMS,
                "missing _meta.io.modelcontextprotocol/clientCapabilities",
                None,
            ));
        }
    }
    Some(match dispatch(data, method, &params, modern).await {
        Ok(result) if modern => respond(modern_result(result)),
        Ok(result) => respond(result),
        Err(Failure::NotFound) => error_message(
            Some(id),
            METHOD_NOT_FOUND,
            &format!("Method not found: {method}"),
            None,
        ),
        Err(Failure::InvalidParams(m)) => error_message(Some(id), INVALID_PARAMS, &m, None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No data at all.
    struct Empty;

    impl Data for Empty {
        async fn sessions(&self) -> Result<Vec<SessionSummary>, String> {
            Ok(Vec::new())
        }
        async fn session(&self, _: Uuid) -> Result<SessionDetail, String> {
            Err("session not found".into())
        }
        async fn shared_tracks(&self) -> Result<Vec<SharedTrack>, String> {
            Ok(Vec::new())
        }
        async fn reference_laps(
            &self,
            _: &str,
            _: &str,
            _: ReferenceScope,
        ) -> Result<Vec<ReferenceLap>, String> {
            Ok(Vec::new())
        }
        async fn compare(&self, _: Vec<Uuid>, _: f32) -> Result<CompareData, String> {
            Err("lap not found".into())
        }
    }

    fn meta() -> Value {
        json!({ META_VERSION: "2026-07-28", META_CAPABILITIES: {} })
    }

    #[tokio::test]
    async fn legacy_handshake_and_tools() {
        let init = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {} } });
        let r = handle_message(&Empty, &init).await.unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert_eq!(handle_message(&Empty, &note).await, None);
        let list = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
        let r = handle_message(&Empty, &list).await.unwrap();
        assert_eq!(r["result"]["tools"].as_array().unwrap().len(), 6);
        assert!(r["result"].get("resultType").is_none());
    }

    #[tokio::test]
    async fn modern_requests() {
        let discover = json!({ "jsonrpc": "2.0", "id": "d", "method": "server/discover",
            "params": { "_meta": meta() } });
        let r = handle_message(&Empty, &discover).await.unwrap();
        assert_eq!(r["result"]["resultType"], "complete");
        assert_eq!(r["result"]["_meta"][META_SERVER_INFO]["name"], "stintory");
        let call = json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "list_sessions", "arguments": {}, "_meta": meta() } });
        let r = handle_message(&Empty, &call).await.unwrap();
        assert_eq!(r["result"]["isError"], false);
        assert_eq!(r["result"]["content"][0]["text"], r#"{"total":0}"#);
        // a failing tool is a result the model can act on
        let call = json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": { "name": "get_session", "arguments": { "session_id": Uuid::nil() },
                        "_meta": meta() } });
        let r = handle_message(&Empty, &call).await.unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert_eq!(r["result"]["content"][0]["text"], "session not found");
    }

    #[tokio::test]
    async fn protocol_errors() {
        let old = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list",
            "params": { "_meta": { META_VERSION: "2099-01-01", META_CAPABILITIES: {} } } });
        let r = handle_message(&Empty, &old).await.unwrap();
        assert_eq!(r["error"]["code"], UNSUPPORTED_VERSION);
        let no_caps = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list",
            "params": { "_meta": { META_VERSION: "2026-07-28" } } });
        assert_eq!(
            handle_message(&Empty, &no_caps).await.unwrap()["error"]["code"],
            INVALID_PARAMS
        );
        let unknown = json!({ "jsonrpc": "2.0", "id": 3, "method": "resources/list" });
        assert_eq!(
            handle_message(&Empty, &unknown).await.unwrap()["error"]["code"],
            METHOD_NOT_FOUND
        );
        let tool = json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": { "name": "delete_everything" } });
        assert_eq!(
            handle_message(&Empty, &tool).await.unwrap()["error"]["code"],
            INVALID_PARAMS
        );
        assert_eq!(
            handle_message(&Empty, &json!([1])).await.unwrap()["error"]["code"],
            INVALID_REQUEST
        );
    }
}
