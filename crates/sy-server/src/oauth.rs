// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! OAuth 2.1 authorization server for MCP clients (AI assistants that
//! connect through OAuth instead of a pasted token, like custom connectors
//! in claude.ai). The server is both the authorization server and the
//! protected resource (`/mcp`), following the MCP authorization spec:
//!
//! - Discovery: protected resource metadata (RFC 9728) at
//!   `/.well-known/oauth-protected-resource[/mcp]`, authorization server
//!   metadata (RFC 8414) at `/.well-known/oauth-authorization-server`; the
//!   401 of `/mcp` points to them.
//! - Clients: client ID metadata documents (`client_id` is an https URL,
//!   fetched with SSRF protection and cached) or dynamic registration
//!   (RFC 7591). Public clients only (no secrets): PKCE protects the code.
//! - Authorization code flow with PKCE (S256 only), exact redirect URIs
//!   (loopback ports may differ, RFC 8252), `iss` in every redirect
//!   (RFC 9207). The user approves in the web UI (`/oauth/authorize`).
//! - Tokens: access tokens for one resource (RFC 8707), valid an hour;
//!   refresh tokens rotated on every use. An approved client is a read-only
//!   device token row, listed and removable on the Account page.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::OnceLock;
use std::time::Duration;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Form, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use deadpool_postgres::GenericClient;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::auth::{Auth, ClientIp, Session, USER_COLUMNS, User};
use crate::crypto;
use crate::devices::url_encode;
use crate::error::{ApiError, ApiResult};
use crate::ratelimit::PER_ADDRESS;
use crate::state::{SharedState, audit};

/// The one scope: read sessions, laps and the analysis.
pub const SCOPE: &str = "laps:read";
/// Prefix of access tokens.
pub const ACCESS_PREFIX: &str = "syo_";
/// Prefix of refresh tokens.
const REFRESH_PREFIX: &str = "syr_";
/// Lifetime of access tokens.
const ACCESS_SECONDS: i32 = 3600;
/// Lifetime of refresh tokens (renewed with every refresh).
pub(crate) const REFRESH_DAYS: i32 = 90;
/// A rotated-out refresh token used again within this is only refused, not
/// taken as stolen: a client that retries a refresh whose answer got lost,
/// or refreshes twice in parallel.
const REUSE_GRACE_SECONDS: i32 = 60;
/// An approval must be exchanged within this.
const CODE_SECONDS: i32 = 300;
/// Client ID metadata documents are fetched again after this.
const DOCUMENT_CACHE_HOURS: i32 = 24;
/// Largest client ID metadata document.
const DOCUMENT_MAX_BYTES: usize = 16 * 1024;
/// Dynamic registrations allowed per address within the rate limit window.
const REGISTRATIONS_PER_WINDOW: u32 = 20;

/// The issuer: the server's public URL.
fn issuer(state: &SharedState) -> String {
    state.config.public_url.trim_end_matches('/').to_owned()
}

/// The protected resource: the MCP endpoint.
pub fn mcp_resource(state: &SharedState) -> String {
    format!("{}/mcp", issuer(state))
}

/// Where clients find the protected resource metadata (for the 401 header).
pub fn resource_metadata_url(state: &SharedState) -> String {
    format!("{}/.well-known/oauth-protected-resource/mcp", issuer(state))
}

// ----- discovery -------------------------------------------------------------

/// `GET /.well-known/oauth-protected-resource[/mcp]` (RFC 9728)
pub async fn protected_resource(State(state): State<SharedState>) -> Json<Value> {
    Json(json!({
        "resource": mcp_resource(&state),
        "authorization_servers": [issuer(&state)],
        "scopes_supported": [SCOPE],
        "bearer_methods_supported": ["header"],
        "resource_name": "Stintory",
    }))
}

/// `GET /.well-known/oauth-authorization-server` (RFC 8414)
pub async fn authorization_server(State(state): State<SharedState>) -> Json<Value> {
    let base = issuer(&state);
    Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "registration_endpoint": format!("{base}/oauth/register"),
        "scopes_supported": [SCOPE],
        "response_types_supported": ["code"],
        "response_modes_supported": ["query"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_methods_supported": ["none"],
        "code_challenge_methods_supported": ["S256"],
        // off: clients fall back to dynamic registration (no fetch needed)
        "client_id_metadata_document_supported": state.config.oauth_client_metadata,
        "authorization_response_iss_parameter_supported": true,
    }))
}

// ----- clients -----------------------------------------------------------------

/// A client that may ask for authorization.
#[derive(Debug, Clone)]
struct Client {
    id: String,
    name: String,
    redirect_uris: Vec<String>,
    /// Known from a client ID metadata document (its host is verified)
    from_document: bool,
}

/// Host of an `http(s)` URL (lowercase, without port), if it has no user
/// info.
fn url_host(url: &str) -> Option<(&str, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let host = if authority.starts_with('[') {
        authority.split_inclusive(']').next().unwrap_or(authority)
    } else {
        authority.rsplit_once(':').map_or(authority, |(h, _)| h)
    };
    Some((scheme, host.to_ascii_lowercase()))
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

/// Redirect URIs must be https or http on a loopback address (MCP spec),
/// without a fragment.
fn valid_redirect_uri(uri: &str) -> bool {
    if uri.len() > 2000 || uri.contains('#') || uri.chars().any(char::is_whitespace) {
        return false;
    }
    match url_host(uri) {
        Some(("https", host)) => !host.is_empty(),
        Some(("http", host)) => is_loopback_host(&host),
        _ => false,
    }
}

/// The URI without the port of its authority (for loopback matching).
fn without_port(uri: &str) -> Option<String> {
    let (scheme, rest) = uri.split_once("://")?;
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let host = if authority.starts_with('[') {
        authority.split_inclusive(']').next().unwrap_or(authority)
    } else {
        authority.rsplit_once(':').map_or(authority, |(h, _)| h)
    };
    Some(format!("{scheme}://{host}{tail}"))
}

/// Exact match; for loopback http redirects the port may differ (RFC 8252
/// §7.3: native apps listen on a free port).
fn redirect_matches(registered: &str, requested: &str) -> bool {
    if registered == requested {
        return true;
    }
    let loopback = |u: &str| matches!(url_host(u), Some(("http", h)) if is_loopback_host(&h));
    loopback(registered)
        && loopback(requested)
        && without_port(registered).is_some()
        && without_port(registered) == without_port(requested)
}

async fn client(state: &SharedState, client_id: &str) -> Result<Client, String> {
    let db = state.conn().await.map_err(|e| e.to_string())?;
    let row = db
        .query_opt(
            "SELECT client_name, redirect_uris, source,
                    fetched_at > now() - make_interval(hours => $2)
             FROM oauth_clients WHERE client_id = $1",
            &[&client_id, &DOCUMENT_CACHE_HOURS],
        )
        .await
        .map_err(|e| e.to_string())?;
    let is_document = client_id.starts_with("https://");
    let fetching = state.config.oauth_client_metadata;
    if let Some(r) = &row {
        let source: &str = r.get(2);
        let fresh: Option<bool> = r.get(3);
        // without fetching, a document fetched earlier stays good
        if source == "registered" || fresh == Some(true) || !fetching {
            return Ok(Client {
                id: client_id.to_owned(),
                name: r.get(0),
                redirect_uris: serde_json::from_value(r.get(1)).unwrap_or_default(),
                from_document: source == "metadata_document",
            });
        }
    }
    if !is_document {
        return Err("unknown client: register it first".into());
    }
    if !fetching {
        return Err(format!(
            "This server doesn't fetch client metadata documents (like {client_id}), it \
             has no internet access. Remove the saved login for this server in the assistant \
             (or add the server again) and connect again: the assistant then registers itself."
        ));
    }
    let client = fetch_metadata_document(client_id).await.map_err(|e| {
        format!(
            "{e}. If this server has no internet access, set STINTORY_OAUTH_CLIENT_METADATA=false \
             and connect again."
        )
    })?;
    db.execute(
        "INSERT INTO oauth_clients (client_id, client_name, redirect_uris, source, fetched_at)
         VALUES ($1, $2, $3, 'metadata_document', now())
         ON CONFLICT (client_id) DO UPDATE SET client_name = $2, redirect_uris = $3,
             source = 'metadata_document', fetched_at = now()",
        &[&client.id, &client.name, &json!(client.redirect_uris)],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(client)
}

/// True for addresses on the public internet (SSRF protection: no
/// loopback, private, link-local, shared or reserved ranges).
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            !(v.is_private()
                || v.is_loopback()
                || v.is_link_local()
                || v.is_broadcast()
                || v.is_documentation()
                || v.is_unspecified()
                || v.is_multicast()
                || o[0] == 0
                || o[0] >= 240
                || (o[0] == 100 && (64..128).contains(&o[1]))
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                // 6to4 relay anycast
                || (o[0] == 192 && o[1] == 88 && o[2] == 99)
                || (o[0] == 198 && (18..20).contains(&o[1])))
        }
        IpAddr::V6(v) => {
            if let Some(v4) = v.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let s = v.segments();
            // transition ranges that carry an IPv4 address (which could be
            // a private one) are refused as a whole
            !(v.is_loopback()
                || v.is_unspecified()
                || v.is_multicast()
                // IPv4-compatible (deprecated), ::/96
                || s[..6] == [0; 6]
                // discard-only
                || (s[0] == 0x0100 && s[1..4] == [0; 3])
                // unique local, link-local, site-local (deprecated)
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
                || (s[0] & 0xffc0) == 0xfec0
                // Teredo, documentation, 6to4
                || (s[0] == 0x2001 && s[1] == 0x0000)
                || (s[0] == 0x2001 && s[1] == 0x0db8)
                || s[0] == 0x2002
                // NAT64, well-known and local-use
                || (s[0] == 0x0064 && s[1] == 0xff9b))
        }
    }
}

/// Resolves only to public addresses.
struct PublicOnly;

impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|a| is_public(a.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(format!("{host} has no public address").into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            // a proxy from the environment would resolve the host itself,
            // past the public-address check of PublicOnly
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .connect_timeout(Duration::from_secs(3))
            .dns_resolver(std::sync::Arc::new(PublicOnly))
            .user_agent(concat!("Stintory/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("HTTP client")
    })
}

/// Fetches and checks a client ID metadata document.
async fn fetch_metadata_document(url: &str) -> Result<Client, String> {
    let bad = |why: &str| format!("the client's metadata document {url} {why}");
    let Some(("https", host)) = url_host(url) else {
        return Err(bad("is not an https URL"));
    };
    let path = url.splitn(4, '/').nth(3).unwrap_or("");
    if path.is_empty()
        || url.contains('#')
        || host.parse::<IpAddr>().is_ok()
        || host.starts_with('[')
    {
        return Err(bad("must be an https URL with a host name and a path"));
    }
    let mut res = http()
        .get(url)
        .header(header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| bad(&format!("could not be fetched ({})", error_chain(&e))))?;
    if res.status() != reqwest::StatusCode::OK {
        return Err(bad(&format!("answered {}", res.status())));
    }
    let mut body = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(|e| bad(&e.to_string()))? {
        body.extend_from_slice(&chunk);
        if body.len() > DOCUMENT_MAX_BYTES {
            return Err(bad("is too large"));
        }
    }
    let doc: Value = serde_json::from_slice(&body).map_err(|_| bad("is not JSON"))?;
    client_from_document(url, &doc).map_err(|why| bad(&why))
}

/// An error with its causes (reqwest hides the interesting part there).
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut msg = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        msg = format!("{msg}: {s}");
        source = s.source();
    }
    msg
}

fn client_from_document(url: &str, doc: &Value) -> Result<Client, String> {
    if doc.get("client_id").and_then(Value::as_str) != Some(url) {
        return Err("has a different client_id".into());
    }
    let name = doc
        .get("client_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or("has no client_name")?;
    let redirect_uris = redirect_uris(doc.get("redirect_uris"))?;
    if let Some(method) = doc
        .get("token_endpoint_auth_method")
        .and_then(Value::as_str)
        && method != "none"
    {
        return Err(format!(
            "asks for {method}; only public clients (\"none\") are supported"
        ));
    }
    Ok(Client {
        id: url.to_owned(),
        name: clean_name(name),
        redirect_uris,
        from_document: true,
    })
}

fn clean_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(64).collect()
}

fn redirect_uris(v: Option<&Value>) -> Result<Vec<String>, String> {
    let uris: Vec<String> = v
        .and_then(Value::as_array)
        .ok_or("has no redirect_uris")?
        .iter()
        .map(|u| {
            u.as_str()
                .map(str::to_owned)
                .ok_or("has a redirect URI that isn't a string")
        })
        .collect::<Result<_, _>>()?;
    if uris.is_empty() || uris.len() > 20 {
        return Err("needs 1 to 20 redirect URIs".into());
    }
    if let Some(bad) = uris.iter().find(|u| !valid_redirect_uri(u)) {
        return Err(format!(
            "has a redirect URI that is neither https nor a loopback address: {bad}"
        ));
    }
    Ok(uris)
}

fn oauth_error(status: StatusCode, error: &str, description: &str) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        Json(json!({ "error": error, "error_description": description })),
    )
        .into_response()
}

/// `POST /oauth/register`: dynamic client registration (RFC 7591), public
/// clients only.
pub async fn register(State(state): State<SharedState>, ip: ClientIp, body: Bytes) -> Response {
    let key = format!("oauth-register:{}", ip.limiter_key());
    if !state.limiter.allowed(&key, REGISTRATIONS_PER_WINDOW) {
        return oauth_error(
            StatusCode::TOO_MANY_REQUESTS,
            "invalid_request",
            "too many registrations",
        );
    }
    state.limiter.fail(&key);
    let Ok(req) = serde_json::from_slice::<Value>(&body) else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            "expected JSON",
        );
    };
    let uris = match redirect_uris(req.get("redirect_uris")) {
        Ok(u) => u,
        Err(why) => {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_redirect_uri",
                &format!("the client {why}"),
            );
        }
    };
    let name = req
        .get("client_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map_or_else(|| "AI assistant".to_owned(), clean_name);
    let id = Uuid::now_v7().to_string();
    let result = async {
        let db = state.conn().await?;
        // registrations nobody used within 30 days
        db.execute(
            "DELETE FROM oauth_clients c WHERE c.source = 'registered'
                AND c.created_at < now() - interval '30 days'
                AND NOT EXISTS (SELECT 1 FROM device_tokens d WHERE d.oauth_client = c.client_id)",
            &[],
        )
        .await?;
        db.execute(
            "INSERT INTO oauth_clients (client_id, client_name, redirect_uris, source)
             VALUES ($1, $2, $3, 'registered')",
            &[&id, &name, &json!(uris)],
        )
        .await?;
        ApiResult::Ok(())
    }
    .await;
    if let Err(e) = result {
        return e.into_response();
    }
    (
        StatusCode::CREATED,
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        Json(json!({
            "client_id": id,
            "client_id_issued_at": time::OffsetDateTime::now_utc().unix_timestamp(),
            "client_name": name,
            "redirect_uris": uris,
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
            "scope": SCOPE,
        })),
    )
        .into_response()
}

// ----- authorization -----------------------------------------------------------

/// The query of `/oauth/authorize`, passed on by the web UI.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AuthorizeParams {
    #[serde(default)]
    response_type: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    redirect_uri: Option<String>,
    #[serde(default)]
    code_challenge: Option<String>,
    #[serde(default)]
    code_challenge_method: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    resource: Option<String>,
}

/// A checked authorization request.
struct Valid {
    client: Client,
    redirect_uri: String,
    challenge: String,
    state: Option<String>,
    resource: String,
}

/// Why an authorization request can't go on.
enum Rejection {
    /// Client or redirect URI unknown: show it, never redirect (RFC 6749 §4.1.2.1)
    Show(String),
    /// Send the browser back with an error
    Redirect(String),
}

/// The redirect URI with the given parameters and `iss`.
fn redirect_to(state: &SharedState, uri: &str, params: &[(&str, &str)]) -> String {
    let mut out = uri.to_owned();
    let mut sep = if uri.contains('?') { '&' } else { '?' };
    let iss = issuer(state);
    for (k, v) in params.iter().copied().chain([("iss", iss.as_str())]) {
        out.push(sep);
        out.push_str(k);
        out.push('=');
        out.push_str(&url_encode(v));
        sep = '&';
    }
    out
}

/// Canonical form of a resource indicator: lowercase scheme and host, no
/// trailing slash. None if it isn't an absolute http(s) URI without a
/// fragment.
fn normalize_resource(r: &str) -> Option<String> {
    if r.contains('#') {
        return None;
    }
    let r = r.trim_end_matches('/');
    let (scheme, rest) = r.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https") {
        return None;
    }
    let end = rest.find('/').unwrap_or(rest.len());
    let (authority, path) = rest.split_at(end);
    Some(format!(
        "{scheme}://{}{path}",
        authority.to_ascii_lowercase()
    ))
}

/// The resource a token is for: the MCP endpoint (also when the client
/// names the server's origin). None for anything else.
fn checked_resource(state: &SharedState, requested: Option<&str>) -> Option<String> {
    let mcp = mcp_resource(state);
    let Some(requested) = requested.filter(|r| !r.is_empty()) else {
        return Some(mcp);
    };
    let r = normalize_resource(requested)?;
    let ours = [normalize_resource(&mcp), normalize_resource(&issuer(state))];
    ours.iter()
        .any(|o| o.as_deref() == Some(r.as_str()))
        .then_some(mcp)
}

async fn validate(state: &SharedState, p: &AuthorizeParams) -> Result<Valid, Rejection> {
    let client_id = p
        .client_id
        .as_deref()
        .filter(|c| !c.is_empty() && c.len() <= 2000)
        .ok_or_else(|| Rejection::Show("The request names no client.".into()))?;
    let client = client(state, client_id).await.map_err(Rejection::Show)?;
    let redirect_uri = match p.redirect_uri.as_deref().filter(|r| !r.is_empty()) {
        Some(r)
            if client
                .redirect_uris
                .iter()
                .any(|reg| redirect_matches(reg, r)) =>
        {
            r.to_owned()
        }
        Some(_) => {
            return Err(Rejection::Show(
                "The redirect address isn't registered for this client.".into(),
            ));
        }
        None if client.redirect_uris.len() == 1 => client.redirect_uris[0].clone(),
        None => {
            return Err(Rejection::Show(
                "The request names no redirect address.".into(),
            ));
        }
    };
    let app_state = p.state.clone().filter(|s| !s.is_empty());
    let fail = |error: &str, description: &str| {
        let mut params = vec![("error", error), ("error_description", description)];
        if let Some(s) = &app_state {
            params.push(("state", s));
        }
        Rejection::Redirect(redirect_to(state, &redirect_uri, &params))
    };
    if app_state.as_ref().is_some_and(|s| s.len() > 1024) {
        return Err(fail("invalid_request", "state is too long"));
    }
    if p.response_type.as_deref() != Some("code") {
        return Err(fail(
            "unsupported_response_type",
            "only the code flow is supported",
        ));
    }
    let challenge = p.code_challenge.clone().unwrap_or_default();
    let challenge_ok = challenge.len() == 43
        && challenge
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if p.code_challenge_method.as_deref() != Some("S256") || !challenge_ok {
        return Err(fail("invalid_request", "PKCE with S256 is required"));
    }
    let resource = checked_resource(state, p.resource.as_deref()).ok_or_else(|| {
        fail(
            "invalid_target",
            "this server only issues tokens for its MCP endpoint",
        )
    })?;
    Ok(Valid {
        client,
        redirect_uri,
        challenge,
        state: app_state,
        resource,
    })
}

/// What the consent page shows.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizeInfo {
    /// The client's own name (unverified)
    client_name: String,
    /// The client id
    client_id: String,
    /// Host of the client's metadata document (verified), if it has one
    client_host: Option<String>,
    /// Where the browser goes back to
    redirect_host: String,
    /// The redirect goes to a program on the user's computer
    local_redirect: bool,
    /// Set instead of the rest when the request is invalid in a way the
    /// client can be told about: send the browser there
    redirect_to: Option<String>,
}

fn browser_only(session: &Session) -> ApiResult<()> {
    if session.device_id.is_some() {
        return Err(ApiError::forbidden("approve AI assistants in the browser"));
    }
    Ok(())
}

/// `GET /api/v1/oauth/authorize?…`: checks the request for the consent page.
pub async fn authorize_info(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Query(p): Query<AuthorizeParams>,
) -> ApiResult<Json<AuthorizeInfo>> {
    browser_only(&session)?;
    match validate(&state, &p).await {
        Ok(v) => {
            let redirect_host = url_host(&v.redirect_uri)
                .map(|(_, h)| h)
                .unwrap_or_default();
            Ok(Json(AuthorizeInfo {
                client_name: v.client.name,
                client_host: v
                    .client
                    .from_document
                    .then(|| url_host(&v.client.id).map(|(_, h)| h))
                    .flatten(),
                client_id: v.client.id,
                local_redirect: is_loopback_host(&redirect_host),
                redirect_host,
                redirect_to: None,
            }))
        }
        Err(Rejection::Show(message)) => Err(ApiError::bad_request(message)),
        Err(Rejection::Redirect(to)) => Ok(Json(AuthorizeInfo {
            client_name: String::new(),
            client_id: String::new(),
            client_host: None,
            redirect_host: String::new(),
            local_redirect: false,
            redirect_to: Some(to),
        })),
    }
}

/// `POST /api/v1/oauth/approve`
#[derive(Debug, Deserialize)]
pub struct ApproveRequest {
    /// The authorization request
    params: AuthorizeParams,
    /// Allowed (true) or denied
    allow: bool,
}

/// Where the browser goes next.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApproveResponse {
    redirect_to: String,
}

/// The user allows or denies the client.
pub async fn approve(
    State(state): State<SharedState>,
    Auth(session): Auth,
    ip: ClientIp,
    Json(req): Json<ApproveRequest>,
) -> ApiResult<Json<ApproveResponse>> {
    browser_only(&session)?;
    let v = match validate(&state, &req.params).await {
        Ok(v) => v,
        Err(Rejection::Show(message)) => return Err(ApiError::bad_request(message)),
        Err(Rejection::Redirect(to)) => return Ok(Json(ApproveResponse { redirect_to: to })),
    };
    let mut params: Vec<(&str, &str)> = Vec::new();
    if !req.allow {
        params.push(("error", "access_denied"));
        if let Some(s) = &v.state {
            params.push(("state", s));
        }
        let redirect_to = redirect_to(&state, &v.redirect_uri, &params);
        return Ok(Json(ApproveResponse { redirect_to }));
    }
    let code = crypto::new_token();
    let db = state.conn().await?;
    db.execute("DELETE FROM oauth_codes WHERE expires_at < now()", &[])
        .await?;
    db.execute(
        "INSERT INTO oauth_codes (code_hash, user_id, client_id, redirect_uri, code_challenge,
                                  resource, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, now() + make_interval(secs => $7))",
        &[
            &crypto::token_hash(&code),
            &session.user.id,
            &v.client.id,
            &v.redirect_uri,
            &v.challenge,
            &v.resource,
            &f64::from(CODE_SECONDS),
        ],
    )
    .await?;
    audit(
        &db,
        Some(session.user.id),
        "oauth_approved",
        ip.0.as_deref(),
        json!({ "client": v.client.id, "name": v.client.name }),
    )
    .await;
    params.push(("code", &code));
    if let Some(s) = &v.state {
        params.push(("state", s));
    }
    Ok(Json(ApproveResponse {
        redirect_to: redirect_to(&state, &v.redirect_uri, &params),
    }))
}

// ----- tokens ------------------------------------------------------------------

fn token_response(access: &str, refresh: &str) -> Response {
    (
        [
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (header::PRAGMA, HeaderValue::from_static("no-cache")),
        ],
        Json(json!({
            "access_token": access,
            "token_type": "Bearer",
            "expires_in": ACCESS_SECONDS,
            "refresh_token": refresh,
            "scope": SCOPE,
        })),
    )
        .into_response()
}

/// A new access token for the grant (`device_tokens` row).
async fn new_access_token(
    db: &impl GenericClient,
    device: Uuid,
    resource: &str,
) -> ApiResult<String> {
    let token = format!("{ACCESS_PREFIX}{}", crypto::new_token());
    db.execute(
        "DELETE FROM oauth_access_tokens WHERE expires_at < now()",
        &[],
    )
    .await?;
    db.execute(
        "INSERT INTO oauth_access_tokens (token_hash, device_id, resource, expires_at)
         VALUES ($1, $2, $3, now() + make_interval(secs => $4))",
        &[
            &crypto::token_hash(&token),
            &device,
            &resource,
            &f64::from(ACCESS_SECONDS),
        ],
    )
    .await?;
    Ok(token)
}

/// `POST /oauth/token`: exchanges a code or a refresh token.
pub async fn token(
    State(state): State<SharedState>,
    ip: ClientIp,
    Form(f): Form<HashMap<String, String>>,
) -> Response {
    let key = format!("oauth-token:{}", ip.limiter_key());
    if !state.limiter.allowed(&key, PER_ADDRESS) {
        return oauth_error(
            StatusCode::TOO_MANY_REQUESTS,
            "invalid_request",
            "too many failed attempts",
        );
    }
    let result = match f.get("grant_type").map(String::as_str) {
        Some("authorization_code") => exchange_code(&state, &ip, &f).await,
        Some("refresh_token") => refresh(&state, &ip, &f).await,
        _ => Err((
            "unsupported_grant_type",
            "use authorization_code or refresh_token",
        )),
    };
    match result {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => e.into_response(),
        Err((error, description)) => {
            if error == "invalid_grant" {
                state.limiter.fail(&key);
            }
            oauth_error(StatusCode::BAD_REQUEST, error, description)
        }
    }
}

type TokenResult = Result<ApiResult<Response>, (&'static str, &'static str)>;

async fn exchange_code(
    state: &SharedState,
    ip: &ClientIp,
    f: &HashMap<String, String>,
) -> TokenResult {
    let get = |k: &str| f.get(k).map(String::as_str).filter(|v| !v.is_empty());
    let (Some(code), Some(client_id), Some(verifier)) =
        (get("code"), get("client_id"), get("code_verifier"))
    else {
        return Err((
            "invalid_request",
            "code, client_id and code_verifier are required",
        ));
    };
    let db = match state.conn().await {
        Ok(db) => db,
        Err(e) => return Ok(Err(e)),
    };
    let row = match db
        .query_opt(
            "DELETE FROM oauth_codes WHERE code_hash = $1 AND expires_at > now()
             RETURNING user_id, client_id, redirect_uri, code_challenge, resource",
            &[&crypto::token_hash(code)],
        )
        .await
    {
        Ok(r) => r,
        Err(e) => return Ok(Err(e.into())),
    };
    let Some(row) = row else {
        return Err(("invalid_grant", "the code is invalid, used or expired"));
    };
    let (user, code_client, redirect, challenge, resource): (Uuid, String, String, String, String) =
        (row.get(0), row.get(1), row.get(2), row.get(3), row.get(4));
    if code_client != client_id {
        return Err(("invalid_grant", "the code was issued to another client"));
    }
    if get("redirect_uri").is_some_and(|r| r != redirect) {
        return Err((
            "invalid_grant",
            "redirect_uri differs from the authorization request",
        ));
    }
    let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    if !(43..=128).contains(&verifier.len()) || expected != challenge {
        return Err(("invalid_grant", "the code verifier doesn't match"));
    }
    if checked_resource(state, get("resource")).as_deref() != Some(resource.as_str()) {
        return Err((
            "invalid_target",
            "resource differs from the authorization request",
        ));
    }
    Ok(async {
        let name: String = db
            .query_opt(
                "SELECT client_name FROM oauth_clients WHERE client_id = $1",
                &[&client_id],
            )
            .await?
            .map_or_else(|| "AI assistant".to_owned(), |r| r.get(0));
        let device = Uuid::now_v7();
        let refresh = format!("{REFRESH_PREFIX}{}", crypto::new_token());
        db.execute(
            "INSERT INTO device_tokens (id, user_id, token_hash, name, scope, oauth_client,
                                        refresh_expires_at)
             VALUES ($1, $2, $3, $4, 'read', $5, now() + make_interval(days => $6))",
            &[
                &device,
                &user,
                &crypto::token_hash(&refresh),
                &name,
                &client_id,
                &REFRESH_DAYS,
            ],
        )
        .await?;
        audit(
            &db,
            Some(user),
            "device_connected",
            ip.0.as_deref(),
            json!({ "device": device, "name": name, "via": "oauth", "client": client_id }),
        )
        .await;
        let access = new_access_token(&db, device, &resource).await?;
        Ok(token_response(&access, &refresh))
    }
    .await)
}

async fn refresh(state: &SharedState, ip: &ClientIp, f: &HashMap<String, String>) -> TokenResult {
    let get = |k: &str| f.get(k).map(String::as_str).filter(|v| !v.is_empty());
    let (Some(refresh), Some(client_id)) = (get("refresh_token"), get("client_id")) else {
        return Err((
            "invalid_request",
            "refresh_token and client_id are required",
        ));
    };
    let Some(resource) = checked_resource(state, get("resource")) else {
        return Err((
            "invalid_target",
            "this server only issues tokens for its MCP endpoint",
        ));
    };
    let next = format!("{REFRESH_PREFIX}{}", crypto::new_token());
    let rotation = rotate(
        state,
        ip,
        &crypto::token_hash(refresh),
        client_id,
        &crypto::token_hash(&next),
    )
    .await;
    let device = match rotation {
        Ok(Rotation::Rotated(device)) => device,
        Ok(Rotation::Unknown) => {
            return Err((
                "invalid_grant",
                "the refresh token is invalid, used or expired",
            ));
        }
        Ok(Rotation::Reused) => {
            return Err((
                "invalid_grant",
                "the refresh token was used before, so it may have been copied: \
                 the connection was ended, please connect again",
            ));
        }
        Err(e) => return Ok(Err(e)),
    };
    let db = match state.conn().await {
        Ok(db) => db,
        Err(e) => return Ok(Err(e)),
    };
    Ok(new_access_token(&db, device, &resource)
        .await
        .map(|access| token_response(&access, &next)))
}

/// What a refresh token turned out to be.
enum Rotation {
    /// Valid: replaced by the next one (grant id)
    Rotated(Uuid),
    /// Unknown, expired, or a just-rotated one coming again (a retry)
    Unknown,
    /// Rotated out a while ago and used again: the grant was ended
    Reused,
}

/// Replaces the refresh token `old` by `next`; the old one is remembered,
/// and if it comes back later, the grant is ended (refresh token reuse
/// detection, OAuth 2.1): the legitimate client and whoever copied the
/// token then both have to connect again, and the user sees it.
async fn rotate(
    state: &SharedState,
    ip: &ClientIp,
    old: &[u8],
    client_id: &str,
    next: &[u8],
) -> ApiResult<Rotation> {
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    let rotated = tx
        .query_opt(
            "UPDATE device_tokens d SET token_hash = $3,
                 refresh_expires_at = now() + make_interval(days => $4)
             FROM users u
             WHERE d.token_hash = $1 AND d.oauth_client = $2 AND d.refresh_expires_at > now()
               AND u.id = d.user_id AND NOT u.disabled
             RETURNING d.id",
            &[&old, &client_id, &next, &REFRESH_DAYS],
        )
        .await?;
    if let Some(row) = rotated {
        let device: Uuid = row.get(0);
        tx.execute(
            "INSERT INTO oauth_used_refresh_tokens (token_hash, device_id) VALUES ($1, $2)
             ON CONFLICT DO NOTHING",
            &[&old, &device],
        )
        .await?;
        tx.commit().await?;
        return Ok(Rotation::Rotated(device));
    }
    let Some(used) = tx
        .query_opt(
            "SELECT d.id, d.user_id, d.name, d.oauth_client
             FROM oauth_used_refresh_tokens r JOIN device_tokens d ON d.id = r.device_id
             WHERE r.token_hash = $1
               AND r.used_at < now() - make_interval(secs => $2)",
            &[&old, &f64::from(REUSE_GRACE_SECONDS)],
        )
        .await?
    else {
        return Ok(Rotation::Unknown);
    };
    let (device, user, name, client): (Uuid, Uuid, String, Option<String>) =
        (used.get(0), used.get(1), used.get(2), used.get(3));
    tx.execute("DELETE FROM device_tokens WHERE id = $1", &[&device])
        .await?;
    tx.commit().await?;
    audit(
        &db,
        Some(user),
        "oauth_refresh_reused",
        ip.as_deref(),
        json!({ "device": device, "name": name, "client": client }),
    )
    .await;
    Ok(Rotation::Reused)
}

/// The user of an OAuth access token for the MCP endpoint (read-only); None
/// if it is unknown, expired, revoked or for another resource.
pub async fn authenticate_access(
    state: &SharedState,
    token: &str,
    ip: &ClientIp,
) -> ApiResult<Option<Session>> {
    let db = state.conn().await?;
    let Some(row) = db
        .query_opt(
            &format!(
                "SELECT d.id, d.last_used_at, a.resource, {USER_COLUMNS}
                 FROM oauth_access_tokens a
                 JOIN device_tokens d ON d.id = a.device_id
                 JOIN users u ON u.id = d.user_id
                 WHERE a.token_hash = $1 AND a.expires_at > now() AND NOT u.disabled"
            ),
            &[&crypto::token_hash(token)],
        )
        .await?
    else {
        return Ok(None);
    };
    let resource: String = row.get(2);
    if resource != mcp_resource(state) {
        return Ok(None);
    }
    let device: Uuid = row.get(0);
    let last_used: Option<time::OffsetDateTime> = row.get(1);
    if last_used.is_none_or(|t| time::OffsetDateTime::now_utc() - t > time::Duration::minutes(5)) {
        db.execute(
            "UPDATE device_tokens SET last_used_at = now(), last_ip = $2 WHERE id = $1",
            &[&device, &ip.0],
        )
        .await?;
    }
    Ok(Some(Session {
        user: User::from_row(&row, 3),
        token_hash: Vec::new(),
        csrf_token: String::new(),
        device_id: Some(device),
        read_only: true,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_uris_are_https_or_loopback() {
        assert!(valid_redirect_uri(
            "https://claude.ai/api/mcp/auth_callback"
        ));
        assert!(valid_redirect_uri("http://localhost:33418/callback"));
        assert!(valid_redirect_uri("http://127.0.0.1/cb"));
        assert!(valid_redirect_uri("http://[::1]:8080/cb"));
        assert!(!valid_redirect_uri("http://example.com/cb"));
        assert!(!valid_redirect_uri("https://example.com/cb#frag"));
        assert!(!valid_redirect_uri("https://user@example.com/cb"));
        assert!(!valid_redirect_uri("myapp://callback"));
        assert!(!valid_redirect_uri("http://localhost.evil.com/cb"));
    }

    #[test]
    fn loopback_ports_may_differ() {
        assert!(redirect_matches(
            "http://127.0.0.1/cb",
            "http://127.0.0.1:51234/cb"
        ));
        assert!(redirect_matches(
            "http://localhost:3000/cb",
            "http://localhost:4000/cb"
        ));
        assert!(!redirect_matches(
            "http://localhost:3000/cb",
            "http://localhost:3000/other"
        ));
        assert!(!redirect_matches(
            "http://localhost/cb",
            "http://127.0.0.1/cb"
        ));
        assert!(!redirect_matches(
            "https://a.example/cb",
            "https://a.example:8443/cb"
        ));
        assert!(redirect_matches(
            "https://a.example/cb",
            "https://a.example/cb"
        ));
    }

    #[test]
    fn only_public_addresses() {
        // 2001:4860:: is Google (not Teredo, which is 2001:0::/32)
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "2606:4700::1111",
            "2001:4860:4860::8888",
            "2a00:1450:4001::1",
        ] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
            "198.18.0.1",
            "224.0.0.1",
            "192.88.99.1",
            "::10.0.0.1",
            "::7f00:1",
            "100::1",
            "fec0::1",
            "2001:0:4136:e378:8000:63bf:3fff:fdd2",
            "2002:a00:1::1",
            "2002:7f00:1::1",
            "64:ff9b::a00:1",
            "64:ff9b:1::a00:1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn resources_are_normalized() {
        assert_eq!(
            normalize_resource("HTTPS://Laps.Example.com/mcp/").as_deref(),
            Some("https://laps.example.com/mcp")
        );
        assert_eq!(normalize_resource("https://x/mcp#a"), None);
        assert_eq!(normalize_resource("laps.example.com/mcp"), None);
    }

    #[test]
    fn metadata_documents_are_checked() {
        let url = "https://app.example/oauth/client.json";
        let doc = json!({
            "client_id": url, "client_name": "Example",
            "redirect_uris": ["http://127.0.0.1/callback"], "token_endpoint_auth_method": "none"
        });
        let c = client_from_document(url, &doc).unwrap();
        assert_eq!(c.name, "Example");
        let mut other = doc.clone();
        other["client_id"] = json!("https://evil.example/client.json");
        assert!(client_from_document(url, &other).is_err());
        let mut bad = doc.clone();
        bad["redirect_uris"] = json!(["http://evil.example/cb"]);
        assert!(client_from_document(url, &bad).is_err());
        let mut secret = doc;
        secret["token_endpoint_auth_method"] = json!("private_key_jwt");
        assert!(client_from_document(url, &secret).is_err());
    }
}
