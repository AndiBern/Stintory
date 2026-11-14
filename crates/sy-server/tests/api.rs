// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! API tests against a real Postgres. Set `STINTORY_TEST_DATABASE_URL` (see
//! docker/compose.dev.yml); without it the tests are skipped. Every test
//! runs in its own schema.

use std::io::Cursor;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sy_model::bundle::BundleWriter;
use sy_model::channel::names;
use sy_model::session::{Conditions, LapFlags};
use sy_model::{
    BlobHash, Channel, ChannelData, GameId, Lap, LapId, LapTelemetry, Origin, Session, SessionId,
    SessionType, Unit,
};
use sy_server::config::Config;
use sy_server::crypto;
use sy_server::state::{Registration, ServerSettings, SharedState};
use tower::ServiceExt;

const ADMIN: &str = "admin";
const ADMIN_PW: &str = "admin password 123";

struct TestServer {
    app: Router,
    state: SharedState,
    url: String,
    schema: String,
    _dir: tempfile::TempDir,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let (url, schema) = (self.url.clone(), self.schema.clone());
        // async cleanup from a sync Drop: own thread, own runtime
        let _ = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    if let Ok((client, conn)) =
                        tokio_postgres::connect(&url, tokio_postgres::NoTls).await
                    {
                        tokio::spawn(conn);
                        // never wait forever for a lock of an unfinished test
                        let _ = client
                            .batch_execute(&format!(
                                "SET lock_timeout = '5s'; DROP SCHEMA {schema} CASCADE"
                            ))
                            .await;
                    }
                });
        })
        .join();
    }
}

async fn server() -> Option<TestServer> {
    server_with(|_| {}).await
}

/// A test server with a changed configuration.
async fn server_with(configure: impl FnOnce(&mut Config)) -> Option<TestServer> {
    let Ok(url) = std::env::var("STINTORY_TEST_DATABASE_URL") else {
        eprintln!("STINTORY_TEST_DATABASE_URL not set, skipped");
        return None;
    };
    let schema = format!("t_{}", uuid::Uuid::new_v4().simple());
    let (client, conn) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
        .await
        .expect("connect to STINTORY_TEST_DATABASE_URL");
    tokio::spawn(conn);
    client
        .batch_execute(&format!("CREATE SCHEMA {schema}"))
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::for_tests(url.clone(), dir.path().to_path_buf());
    config.admin_username = Some(ADMIN.into());
    config.admin_password = Some(ADMIN_PW.into());
    configure(&mut config);
    let state = sy_server::init(config, Some(&schema)).await.unwrap();
    Some(TestServer {
        app: sy_server::app(state.clone()),
        state,
        url,
        schema,
        _dir: dir,
    })
}

/// A browser: keeps the session cookie and the CSRF token.
struct Browser<'a> {
    server: &'a TestServer,
    cookie: Option<String>,
    csrf: Option<String>,
    /// Device token (desktop app) instead of a cookie
    bearer: Option<String>,
}

impl<'a> Browser<'a> {
    fn new(server: &'a TestServer) -> Self {
        Self {
            server,
            cookie: None,
            csrf: None,
            bearer: None,
        }
    }

    fn device(server: &'a TestServer, token: &str) -> Self {
        Self {
            bearer: Some(token.to_owned()),
            ..Self::new(server)
        }
    }

    async fn send(&mut self, req: Request<Body>) -> (StatusCode, Value) {
        let res = self.server.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        for v in res.headers().get_all(header::SET_COOKIE) {
            let v = v.to_str().unwrap();
            let pair = v.split(';').next().unwrap();
            if pair.ends_with('=') || v.contains("Max-Age=0") {
                self.cookie = None;
            } else {
                self.cookie = Some(pair.to_owned());
            }
        }
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        if let Some(csrf) = body.get("csrfToken").and_then(Value::as_str) {
            self.csrf = Some(csrf.to_owned());
        }
        (status, body)
    }

    fn request(&self, method: &str, path: &str) -> axum::http::request::Builder {
        let mut b = Request::builder()
            .method(method)
            .uri(format!("/api/v1{path}"));
        if let Some(c) = &self.cookie {
            b = b.header(header::COOKIE, c);
        }
        if let Some(t) = &self.bearer {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        if method != "GET"
            && let Some(t) = &self.csrf
        {
            b = b.header("x-csrf-token", t);
        }
        b
    }

    async fn get(&mut self, path: &str) -> (StatusCode, Value) {
        let req = self.request("GET", path).body(Body::empty()).unwrap();
        self.send(req).await
    }

    async fn call(&mut self, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let req = self
            .request(method, path)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        self.send(req).await
    }

    async fn post(&mut self, path: &str, body: Value) -> (StatusCode, Value) {
        self.call("POST", path, body).await
    }

    async fn login(&mut self, user: &str, password: &str) -> (StatusCode, Value) {
        self.post(
            "/auth/login",
            json!({ "username": user, "password": password }),
        )
        .await
    }
}

fn base32_decode(s: &str) -> Vec<u8> {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let (mut buf, mut bits, mut out) = (0u32, 0u32, Vec::new());
    for c in s.bytes() {
        buf = (buf << 5) | A.iter().position(|&x| x == c).unwrap() as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    out
}

fn code(secret: &[u8], step: u64) -> String {
    format!("{:06}", crypto::totp_code(secret, step))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_status_csrf_and_logout() {
    let Some(s) = server().await else { return };
    let mut b = Browser::new(&s);

    let (st, body) = b.get("/auth/status").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["user"], Value::Null);
    assert_eq!(b.get("/sessions").await.0, StatusCode::UNAUTHORIZED);

    let (st, body) = b.login(ADMIN, "wrong password!").await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "wrong username or password");
    let (st, body) = b.login("nobody", "wrong password!").await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "wrong username or password", "same message");

    let (st, body) = b.login("Admin", ADMIN_PW).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "ok");
    assert_eq!(body["user"]["isAdmin"], true);
    assert!(b.cookie.is_some());

    let (_, body) = b.get("/auth/status").await;
    assert_eq!(body["user"]["username"], "admin");
    assert_eq!(b.get("/sessions").await, (StatusCode::OK, json!([])));

    // a changing request without the CSRF token is rejected
    let csrf = b.csrf.take();
    let (st, body) = b.call("PATCH", "/me", json!({ "displayName": "X" })).await;
    assert_eq!(
        (st, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("csrf"))
    );
    b.csrf = csrf;
    let (st, _) = b
        .call("PATCH", "/me", json!({ "displayName": "The Admin" }))
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert_eq!(b.get("/me").await.1["displayName"], "The Admin");

    assert_eq!(
        b.post("/auth/logout", json!({})).await.0,
        StatusCode::NO_CONTENT
    );
    assert!(b.cookie.is_none());
    assert_eq!(b.get("/sessions").await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_from_another_site_is_rejected() {
    let Some(s) = server().await else { return };
    let mut b = Browser::new(&s);
    let req = b
        .request("POST", "/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .header("origin", "https://evil.example")
        .body(Body::from(
            json!({ "username": ADMIN, "password": ADMIN_PW }).to_string(),
        ))
        .unwrap();
    assert_eq!(b.send(req).await.0, StatusCode::FORBIDDEN);
    let req = b
        .request("POST", "/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .header("origin", "http://localhost:8080")
        .body(Body::from(
            json!({ "username": ADMIN, "password": ADMIN_PW }).to_string(),
        ))
        .unwrap();
    assert_eq!(b.send(req).await.0, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn totp_setup_login_and_recovery_codes() {
    let Some(s) = server().await else { return };
    let mut b = Browser::new(&s);
    b.login(ADMIN, ADMIN_PW).await;

    let (st, setup) = b.post("/me/totp/setup", json!({})).await;
    assert_eq!(st, StatusCode::OK, "{setup}");
    assert!(setup["qrSvg"].as_str().unwrap().contains("<svg"));
    assert!(
        setup["uri"]
            .as_str()
            .unwrap()
            .starts_with("otpauth://totp/")
    );
    let secret = base32_decode(setup["secret"].as_str().unwrap());
    let now = crypto::totp_step_now();

    let (st, body) = b
        .post("/me/totp/confirm", json!({ "code": "000000" }))
        .await;
    assert_eq!(
        (st, body["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("wrong_code"))
    );
    let (st, body) = b
        .post("/me/totp/confirm", json!({ "code": code(&secret, now) }))
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let recovery: Vec<String> = serde_json::from_value(body["recoveryCodes"].clone()).unwrap();
    assert_eq!(recovery.len(), 10);
    b.post("/auth/logout", json!({})).await;

    // password alone is not enough anymore
    let (st, body) = b.login(ADMIN, ADMIN_PW).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["status"], "secondFactor");
    assert!(b.cookie.is_none());
    let challenge = body["challenge"].as_str().unwrap().to_owned();
    let second = |code: String| json!({ "challenge": challenge, "code": code });

    let (st, _) = b
        .post("/auth/login/second-factor", second("123456".into()))
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    // the code used for the setup can't be used again (replay)
    let (st, _) = b
        .post("/auth/login/second-factor", second(code(&secret, now)))
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, body) = b
        .post("/auth/login/second-factor", second(code(&secret, now + 1)))
        .await;
    assert_eq!(
        (st, &body["status"]),
        (StatusCode::OK, &json!("ok")),
        "{body}"
    );
    assert_eq!(b.get("/sessions").await.0, StatusCode::OK);
    b.post("/auth/logout", json!({})).await;

    // a recovery code works once (any case, with or without dash)
    let (_, body) = b.login(ADMIN, ADMIN_PW).await;
    let challenge = body["challenge"].as_str().unwrap().to_owned();
    let (st, _) = b
        .post(
            "/auth/login/second-factor",
            json!({ "challenge": challenge, "code": recovery[0].to_uppercase() }),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    b.post("/auth/logout", json!({})).await;
    let (_, body) = b.login(ADMIN, ADMIN_PW).await;
    let challenge = body["challenge"].as_str().unwrap().to_owned();
    let (st, _) = b
        .post(
            "/auth/login/second-factor",
            json!({ "challenge": challenge, "code": recovery[0] }),
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "used up");

    // the challenge dies after too many wrong codes
    for _ in 0..5 {
        b.post(
            "/auth/login/second-factor",
            json!({ "challenge": challenge, "code": "000000" }),
        )
        .await;
    }
    let (st, body) = b
        .post(
            "/auth/login/second-factor",
            json!({ "challenge": challenge, "code": recovery[1] }),
        )
        .await;
    assert_eq!(
        (st, body["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("challenge_expired"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_creates_user_who_must_change_password() {
    let Some(s) = server().await else { return };
    let mut admin = Browser::new(&s);
    admin.login(ADMIN, ADMIN_PW).await;
    let (st, created) = admin
        .post(
            "/admin/users",
            json!({ "username": "Max", "displayName": "Max M." }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{created}");
    let temp = created["temporaryPassword"].as_str().unwrap().to_owned();
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(
        admin
            .post("/admin/users", json!({ "username": "max" }))
            .await
            .0,
        StatusCode::CONFLICT
    );

    let mut max = Browser::new(&s);
    let (_, body) = max.login("max", &temp).await;
    assert_eq!(body["user"]["mustChangePassword"], true);
    let (st, body) = max.get("/sessions").await;
    assert_eq!(
        (st, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("password_change_required"))
    );
    let (st, _) = max
        .post(
            "/me/password",
            json!({ "currentPassword": temp, "newPassword": "short" }),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = max
        .post(
            "/me/password",
            json!({ "currentPassword": temp, "newPassword": "my own password 1" }),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert_eq!(max.get("/sessions").await.0, StatusCode::OK);
    // not an admin
    assert_eq!(max.get("/admin/users").await.0, StatusCode::FORBIDDEN);

    // disabling ends the sessions and blocks the login
    let (st, _) = admin
        .call(
            "PATCH",
            &format!("/admin/users/{id}"),
            json!({ "disabled": true }),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(max.get("/sessions").await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        max.login("max", "my own password 1").await.0,
        StatusCode::UNAUTHORIZED
    );

    // the last admin can't remove their own rights
    let me = admin.get("/me").await.1["id"].as_str().unwrap().to_owned();
    let (st, _) = admin
        .call(
            "PATCH",
            &format!("/admin/users/{me}"),
            json!({ "isAdmin": false }),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (st, log) = admin.get("/admin/audit").await;
    assert_eq!(st, StatusCode::OK);
    let events: Vec<&str> = log
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event"].as_str().unwrap())
        .collect();
    for e in [
        "admin_user_created",
        "password_changed",
        "admin_user_changed",
        "login",
    ] {
        assert!(events.contains(&e), "{e} missing in {events:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_logins_are_rate_limited() {
    let Some(s) = server().await else { return };
    let mut b = Browser::new(&s);
    for _ in 0..sy_server::ratelimit::PER_ACCOUNT {
        assert_eq!(
            b.login(ADMIN, "wrong password!").await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        b.login(ADMIN, ADMIN_PW).await.0,
        StatusCode::TOO_MANY_REQUESTS,
        "even the right password"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enforced_two_factor_restricts_users_without_it() {
    let Some(s) = server().await else { return };
    let mut b = Browser::new(&s);
    b.login(ADMIN, ADMIN_PW).await;
    // the admin can't enforce it without having it
    let (st, _) = b
        .call(
            "PUT",
            "/admin/settings",
            json!({ "registration": "closed", "enforce2fa": true, "allowShareEveryone": true }),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    ServerSettings {
        enforce_2fa: true,
        ..Default::default()
    }
    .save(&s.state.conn().await.unwrap())
    .await
    .unwrap();
    let (st, body) = b.get("/sessions").await;
    assert_eq!(
        (st, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("two_factor_required"))
    );
    assert_eq!(b.get("/me").await.1["twoFactorRequired"], true);
    // the setup itself is allowed
    assert_eq!(b.post("/me/totp/setup", json!({})).await.0, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registration_follows_the_setting() {
    let Some(s) = server().await else { return };
    let mut b = Browser::new(&s);
    let req = json!({ "username": "newbie", "password": "a long password 1" });
    assert_eq!(
        b.post("/auth/register", req.clone()).await.0,
        StatusCode::FORBIDDEN
    );
    ServerSettings {
        registration: Registration::Open,
        ..Default::default()
    }
    .save(&s.state.conn().await.unwrap())
    .await
    .unwrap();
    assert_eq!(b.get("/auth/status").await.1["registrationOpen"], true);
    let (st, body) = b.post("/auth/register", req.clone()).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["user"]["username"], "newbie");
    assert_eq!(b.get("/sessions").await.0, StatusCode::OK);
    assert_eq!(
        Browser::new(&s).post("/auth/register", req).await.0,
        StatusCode::CONFLICT
    );
}

/// A session with two laps and their telemetry files.
fn sample() -> (Session, Vec<(Lap, Vec<u8>)>) {
    let session = Session {
        id: SessionId::new(),
        game: GameId::Iracing,
        game_version: None,
        track_key: "iracing:439".into(),
        track_name: "Winton".into(),
        car_key: "iracing:67".into(),
        car_name: "MX-5".into(),
        session_type: SessionType::Practice,
        started_at: time::macros::datetime!(2026-11-10 16:00 UTC),
        ended_at: None,
        conditions: Conditions::default(),
        setup: None,
        session_info: None,
        origin: Origin::Own,
    };
    let mut laps = Vec::new();
    for n in 1..=2u32 {
        let samples = 100;
        let t = LapTelemetry::new(10.0)
            .with_channel(Channel::new(
                names::TIME,
                Unit::Second,
                ChannelData::F32((0..samples).map(|i| i as f32 / 10.0).collect()),
            ))
            .unwrap()
            .with_channel(Channel::new(
                names::DIST,
                Unit::Meter,
                ChannelData::F32((0..samples).map(|i| i as f32 * 10.0).collect()),
            ))
            .unwrap()
            .with_channel(Channel::new(
                names::SPEED,
                Unit::MeterPerSecond,
                ChannelData::F32(vec![50.0 + n as f32; samples]),
            ))
            .unwrap()
            // one right-hander from 300 to 450 m (radius ~130 m)
            .with_channel(Channel::new(
                names::G_LAT,
                Unit::MeterPerSecondSquared,
                ChannelData::F32(
                    (0..samples)
                        .map(|i| if (30..45).contains(&i) { 20.0 } else { 0.0 })
                        .collect(),
                ),
            ))
            .unwrap();
        let bytes = t.to_parquet().unwrap();
        let lap = Lap {
            id: LapId::new(),
            session_id: session.id,
            number: n,
            lap_time_ms: Some(10_000 - n),
            sector_times_ms: vec![],
            valid: true,
            invalid_reason: None,
            flags: LapFlags::default(),
            blob_hash: BlobHash::of(&bytes),
            sample_rate_hz: 10.0,
            sample_count: samples as u32,
            start_time_s: Some(f64::from(n) * 10.0),
        };
        laps.push((lap, bytes));
    }
    (session, laps)
}

/// `sample()` as an .stintory file.
fn bundle() -> (Vec<u8>, Session, Vec<Lap>) {
    let (session, laps) = sample();
    let mut w = BundleWriter::new(Cursor::new(Vec::new()));
    w.add_session(&session).unwrap();
    for (lap, bytes) in &laps {
        w.add_lap(lap, bytes).unwrap();
    }
    let file = w.finish("test", Some("Andi")).unwrap().into_inner();
    (file, session, laps.into_iter().map(|(l, _)| l).collect())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_list_compare_and_delete() {
    let Some(s) = server().await else { return };
    let mut b = Browser::new(&s);
    b.login(ADMIN, ADMIN_PW).await;
    let (file, session, laps) = bundle();
    let upload = |b: &Browser<'_>, file: Vec<u8>| {
        b.request("POST", "/import")
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(Body::from(file))
            .unwrap()
    };

    let req = upload(&b, file.clone());
    let (st, body) = b.send(req).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({ "files": 1, "sessions": 1, "laps": 2, "skippedLaps": 0 })
    );
    let req = upload(&b, file.clone());
    assert_eq!(b.send(req).await.1["skippedLaps"], 2, "idempotent");
    let req = upload(&b, b"not a zip".to_vec());
    assert_eq!(b.send(req).await.0, StatusCode::BAD_REQUEST);

    let (_, list) = b.get("/sessions").await;
    assert_eq!(list[0]["id"], session.id.to_string());
    assert_eq!(list[0]["lapCount"], 2);
    assert_eq!(list[0]["bestLapMs"], 9_998);
    assert_eq!(
        list[0]["origin"],
        json!({ "kind": "imported", "driverName": "Andi" })
    );

    let (st, detail) = b.get(&format!("/sessions/{}", session.id)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(detail["laps"].as_array().unwrap().len(), 2);

    let (st, cmp) = b
        .post(
            "/analysis/compare",
            json!({ "lapIds": [laps[1].id, laps[0].id] }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{cmp}");
    assert_eq!(cmp["laps"][0]["lapId"], laps[1].id.to_string());
    assert!(cmp["distM"].as_array().unwrap().len() > 900);

    // another user sees nothing of it
    let (_, created) = b.post("/admin/users", json!({ "username": "other" })).await;
    let temp = created["temporaryPassword"].as_str().unwrap().to_owned();
    let mut other = Browser::new(&s);
    other.login("other", &temp).await;
    other
        .post(
            "/me/password",
            json!({ "currentPassword": temp, "newPassword": "other password 12" }),
        )
        .await;
    assert_eq!(other.get("/sessions").await.1, json!([]));
    assert_eq!(
        other.get(&format!("/sessions/{}", session.id)).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        other
            .post("/analysis/compare", json!({ "lapIds": [laps[0].id] }))
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    // deleting removes the telemetry files
    assert!(s.state.blobs.contains(&laps[0].blob_hash));
    let (st, _) = b
        .call("DELETE", &format!("/sessions/{}", session.id), json!({}))
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(!s.state.blobs.contains(&laps[0].blob_hash));
    assert_eq!(b.get("/sessions").await.1, json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_api_paths_are_json_404() {
    let Some(s) = server().await else { return };
    let (st, body) = Browser::new(&s).get("/nope").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert!(body["error"].is_string());
}

fn pkce() -> (String, String) {
    use base64::Engine;
    use sha2::Digest;
    let verifier = crypto::new_token(); // 43 characters
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// The query parameter `name` of a URL.
fn param(url: &str, name: &str) -> String {
    url.split(['?', '&'])
        .find_map(|p| p.strip_prefix(&format!("{name}=")))
        .unwrap()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_login_and_revocation() {
    let Some(s) = server().await else { return };
    let mut b = Browser::new(&s);
    b.login(ADMIN, ADMIN_PW).await;
    let (verifier, challenge) = pkce();
    let approve = |redirect: &str, challenge: &str| {
        json!({
            "redirectUri": redirect, "codeChallenge": challenge,
            "state": "xyz 1", "deviceName": "Rig PC"
        })
    };

    // only loopback redirects
    let (st, _) = b
        .post(
            "/device-login/approve",
            approve("http://evil.example:5000/cb", &challenge),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // a wrong verifier uses the code up
    let (st, body) = b
        .post(
            "/device-login/approve",
            approve("http://127.0.0.1:49200/callback", &challenge),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let to = body["redirectTo"].as_str().unwrap();
    assert!(
        to.starts_with("http://127.0.0.1:49200/callback?code="),
        "{to}"
    );
    assert_eq!(param(to, "state"), "xyz%201");
    let code = param(to, "code");
    let mut app = Browser::new(&s);
    let (st, body) = app
        .post(
            "/device-login/token",
            json!({ "code": code, "codeVerifier": pkce().0 }),
        )
        .await;
    assert_eq!(
        (st, body["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_grant"))
    );
    let (st, _) = app
        .post(
            "/device-login/token",
            json!({ "code": code, "codeVerifier": verifier }),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "single use");

    // the real flow
    let (_, body) = b
        .post(
            "/device-login/approve",
            approve("http://127.0.0.1:49200/callback", &challenge),
        )
        .await;
    let code = param(body["redirectTo"].as_str().unwrap(), "code");
    let (st, body) = app
        .post(
            "/device-login/token",
            json!({ "code": code, "codeVerifier": verifier }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let token = body["token"].as_str().unwrap().to_owned();
    assert!(token.starts_with("syd_"));
    assert_eq!(body["device"]["name"], "Rig PC");
    assert_eq!(body["user"]["username"], ADMIN);

    let mut dev = Browser::device(&s, &token);
    let (st, me) = dev.get("/devices/current").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(me["device"]["name"], "Rig PC");
    assert_eq!(dev.get("/sessions").await.0, StatusCode::OK);
    // no account or admin pages with a device token
    assert_eq!(dev.get("/admin/users").await.0, StatusCode::FORBIDDEN);
    assert_eq!(dev.get("/me").await.0, StatusCode::UNAUTHORIZED);
    // and no CSRF token needed: it's not a cookie
    let (st, _) = dev.post("/sync/push", json!({})).await;
    assert_eq!(st, StatusCode::OK);

    // the account page lists it; revoking stops it
    let (_, devices) = b.get("/me/devices").await;
    let id = devices[0]["id"].as_str().unwrap().to_owned();
    assert!(devices[0]["lastUsedAt"].is_string());
    let (st, _) = b
        .call("DELETE", &format!("/me/devices/{id}"), json!({}))
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, body) = dev.get("/sessions").await;
    assert_eq!(
        (st, body["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("device_revoked"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_between_two_devices() {
    let Some(s) = server().await else { return };
    let mut b = Browser::new(&s);
    b.login(ADMIN, ADMIN_PW).await;
    let token = || async {
        let (st, body) = Browser {
            server: &s,
            cookie: b.cookie.clone(),
            csrf: b.csrf.clone(),
            bearer: None,
        }
        .post("/me/devices", json!({ "name": "pc" }))
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        body["token"].as_str().unwrap().to_owned()
    };
    let (ta, tb) = (token().await, token().await);
    let (mut a, mut bdev) = (Browser::device(&s, &ta), Browser::device(&s, &tb));
    let (session, laps) = sample();

    // device A pushes: blobs first
    let hashes: Vec<String> = laps.iter().map(|(l, _)| l.blob_hash.to_string()).collect();
    let (_, body) = a
        .post("/sync/blobs/missing", json!({ "hashes": hashes }))
        .await;
    assert_eq!(body["missing"].as_array().unwrap().len(), 2);
    let (st, _) = a
        .post(
            "/sync/push",
            json!({ "sessions": [session], "laps": [laps[0].0] }),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "telemetry not uploaded yet");
    for (lap, bytes) in &laps {
        let req = a
            .request("PUT", &format!("/sync/blobs/{}", lap.blob_hash))
            .body(Body::from(bytes.clone()))
            .unwrap();
        assert_eq!(a.send(req).await.0, StatusCode::NO_CONTENT);
    }
    let req = a
        .request("PUT", &format!("/sync/blobs/{}", laps[0].0.blob_hash))
        .body(Body::from(b"tampered".to_vec()))
        .unwrap();
    assert_eq!(a.send(req).await.0, StatusCode::BAD_REQUEST);
    let all_laps: Vec<&Lap> = laps.iter().map(|(l, _)| l).collect();
    let (st, body) = a
        .post(
            "/sync/push",
            json!({ "sessions": [session], "laps": all_laps }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({ "sessions": 1, "laps": 2, "deletedSessions": 0 })
    );

    // device B pulls everything
    let (_, ch) = bdev.get("/sync/changes?since=0").await;
    assert_eq!(ch["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(ch["laps"].as_array().unwrap().len(), 2);
    let got: Session = serde_json::from_value(ch["sessions"][0].clone()).unwrap();
    assert_eq!(got, session);
    let got: Lap = serde_json::from_value(ch["laps"][0].clone()).unwrap();
    assert!(laps.iter().any(|(l, _)| *l == got));
    let cursor = ch["cursor"].as_i64().unwrap();
    assert_eq!(ch["more"], false);
    let (_, ch) = bdev.get(&format!("/sync/changes?since={cursor}")).await;
    assert!(
        ch["sessions"].as_array().unwrap().is_empty() && ch["laps"].as_array().unwrap().is_empty()
    );
    // paging: one change per page, laps bring their session along
    let (_, page) = bdev.get("/sync/changes?since=0&limit=1").await;
    assert_eq!(page["more"], true);

    // telemetry download, only for own laps
    let req = bdev
        .request("GET", &format!("/sync/blobs/{}", laps[0].0.blob_hash))
        .body(Body::empty())
        .unwrap();
    let res = s.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(bytes.as_ref(), laps[0].1.as_slice());
    let (_, created) = b.post("/admin/users", json!({ "username": "other" })).await;
    let temp = created["temporaryPassword"].as_str().unwrap().to_owned();
    let mut other = Browser::new(&s);
    other.login("other", &temp).await;
    other
        .post(
            "/me/password",
            json!({ "currentPassword": temp, "newPassword": "other password 12" }),
        )
        .await;
    assert_eq!(
        other
            .get(&format!("/sync/blobs/{}", laps[0].0.blob_hash))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        other.get("/sync/changes?since=0").await.1["laps"],
        json!([])
    );

    // an update reaches B
    let mut ended = session.clone();
    ended.ended_at = Some(time::macros::datetime!(2026-11-10 16:30 UTC));
    a.post("/sync/push", json!({ "sessions": [ended] })).await;
    let (_, ch) = bdev.get(&format!("/sync/changes?since={cursor}")).await;
    let got: Session = serde_json::from_value(ch["sessions"][0].clone()).unwrap();
    assert_eq!(got.ended_at, ended.ended_at);
    let cursor = ch["cursor"].as_i64().unwrap();

    // a deletion reaches B, the telemetry is removed from the server
    let (_, body) = a
        .post("/sync/push", json!({ "deletedSessions": [session.id] }))
        .await;
    assert_eq!(body["deletedSessions"], 1);
    let (_, ch) = bdev.get(&format!("/sync/changes?since={cursor}")).await;
    assert_eq!(ch["deletedSessions"], json!([session.id]));
    assert!(!s.state.blobs.contains(&laps[0].0.blob_hash));
    assert_eq!(b.get("/sessions").await.1, json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn passkeys_as_second_factor_and_passwordless() {
    use webauthn_authenticator_rs::WebauthnAuthenticator;
    use webauthn_authenticator_rs::softpasskey::SoftPasskey;
    use webauthn_rs_proto::{CreationChallengeResponse, RequestChallengeResponse};

    let Some(s) = server().await else { return };
    let origin = url::Url::parse("http://localhost:8080").unwrap();
    // a software authenticator; it can't do user verification, so it claims it
    let mut key = SoftPasskey::new(true);
    let mut b = Browser::new(&s);
    b.login(ADMIN, ADMIN_PW).await;

    // adding needs the password
    let (st, _) = b
        .post(
            "/me/passkeys/register/start",
            json!({ "password": "wrong!", "name": "x" }),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, start) = b
        .post(
            "/me/passkeys/register/start",
            json!({ "password": ADMIN_PW, "name": "Soft key" }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{start}");
    let options: CreationChallengeResponse =
        serde_json::from_value(start["options"].clone()).unwrap();
    let credential = key.do_registration(origin.clone(), options).unwrap();
    let (st, added) = b
        .post(
            "/me/passkeys/register/finish",
            json!({ "challengeId": start["challengeId"], "credential": credential }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{added}");
    assert_eq!(added["name"], "Soft key");
    // the same ceremony can't be finished twice
    let (st, body) = b
        .post(
            "/me/passkeys/register/finish",
            json!({ "challengeId": start["challengeId"], "credential": credential }),
        )
        .await;
    assert_eq!(
        (st, body["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("passkey_expired"))
    );
    assert_eq!(b.get("/me").await.1["hasPasskey"], true);

    // a passkey satisfies an enforced 2FA setting
    ServerSettings {
        enforce_2fa: true,
        ..Default::default()
    }
    .save(&s.state.conn().await.unwrap())
    .await
    .unwrap();
    assert_eq!(b.get("/sessions").await.0, StatusCode::OK);
    b.post("/auth/logout", json!({})).await;

    // password + passkey
    let (_, body) = b.login(ADMIN, ADMIN_PW).await;
    assert_eq!(body["status"], "secondFactor");
    assert_eq!(body["methods"], json!(["passkey"]));
    let challenge = body["challenge"].as_str().unwrap().to_owned();
    let (st, start) = b
        .post(
            "/auth/login/second-factor/passkey/start",
            json!({ "challenge": challenge }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{start}");
    let options: RequestChallengeResponse =
        serde_json::from_value(start["options"].clone()).unwrap();
    let assertion = key.do_authentication(origin.clone(), options).unwrap();
    let (st, body) = b
        .post(
            "/auth/login/second-factor/passkey/finish",
            json!({ "challengeId": start["challengeId"], "credential": assertion, "challenge": challenge }),
        )
        .await;
    assert_eq!(
        (st, &body["status"]),
        (StatusCode::OK, &json!("ok")),
        "{body}"
    );
    assert_eq!(b.get("/sessions").await.0, StatusCode::OK);
    b.post("/auth/logout", json!({})).await;

    // passwordless: the browser picks the passkey (the software key can't
    // offer discoverable credentials, so we name it, like a browser would)
    let mut nobody = Browser::new(&s);
    let (st, start) = nobody.post("/auth/passkey/start", json!({})).await;
    assert_eq!(st, StatusCode::OK, "{start}");
    let mut options: RequestChallengeResponse =
        serde_json::from_value(start["options"].clone()).unwrap();
    assert!(options.public_key.allow_credentials.is_empty());
    let cred_id = credential.raw_id.clone();
    options
        .public_key
        .allow_credentials
        .push(webauthn_rs_proto::AllowCredentials {
            type_: "public-key".into(),
            id: cred_id,
            transports: None,
        });
    let assertion = key.do_authentication(origin.clone(), options).unwrap();
    let (st, body) = nobody
        .post(
            "/auth/passkey/finish",
            json!({ "challengeId": start["challengeId"], "credential": assertion }),
        )
        .await;
    assert_eq!(
        (st, &body["status"]),
        (StatusCode::OK, &json!("ok")),
        "{body}"
    );
    assert_eq!(body["user"]["username"], ADMIN);
    // replaying the same assertion with a new ceremony fails
    let (_, start2) = Browser::new(&s)
        .post("/auth/passkey/start", json!({}))
        .await;
    let (st, _) = Browser::new(&s)
        .post(
            "/auth/passkey/finish",
            json!({ "challengeId": start2["challengeId"], "credential": assertion }),
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // the last second factor can't be removed while 2FA is required
    let (_, list) = nobody.get("/me/passkeys").await;
    let id = list[0]["id"].as_str().unwrap().to_owned();
    assert!(list[0]["lastUsedAt"].is_string());
    let (st, _) = nobody
        .call("DELETE", &format!("/me/passkeys/{id}"), json!({}))
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    ServerSettings::default()
        .save(&s.state.conn().await.unwrap())
        .await
        .unwrap();
    let (st, _) = nobody
        .call("DELETE", &format!("/me/passkeys/{id}"), json!({}))
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    nobody.post("/auth/logout", json!({})).await;
    assert_eq!(nobody.login(ADMIN, ADMIN_PW).await.1["status"], "ok");
}

// ----- teams and sharing (docs/ARCHITECTURE.md §9.2) -------------------------

/// Creates an account and logs it in (with its own password set).
async fn user<'a>(s: &'a TestServer, admin: &mut Browser<'_>, name: &str) -> (Browser<'a>, String) {
    let (st, created) = admin
        .post("/admin/users", json!({ "username": name }))
        .await;
    assert_eq!(st, StatusCode::OK, "{created}");
    let temp = created["temporaryPassword"].as_str().unwrap().to_owned();
    let mut b = Browser::new(s);
    b.login(name, &temp).await;
    let (st, _) = b
        .post(
            "/me/password",
            json!({ "currentPassword": temp, "newPassword": "a long password 1" }),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    (b, created["id"].as_str().unwrap().to_owned())
}

/// Stores `sample()` as a session the user recorded (sync push).
async fn push_sample(b: &mut Browser<'_>) -> (Session, Vec<Lap>) {
    push_sample_with(b, &[]).await
}

/// `push_sample` with other lap times (as many as given).
async fn push_sample_with(b: &mut Browser<'_>, times: &[u32]) -> (Session, Vec<Lap>) {
    let (session, mut laps) = sample();
    for ((lap, _), ms) in laps.iter_mut().zip(times) {
        lap.lap_time_ms = Some(*ms);
    }
    for (lap, bytes) in &laps {
        let req = b
            .request("PUT", &format!("/sync/blobs/{}", lap.blob_hash))
            .body(Body::from(bytes.clone()))
            .unwrap();
        assert_eq!(b.send(req).await.0, StatusCode::NO_CONTENT);
    }
    let laps: Vec<Lap> = laps.into_iter().map(|(l, _)| l).collect();
    let (st, body) = b
        .post("/sync/push", json!({ "sessions": [session], "laps": laps }))
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    (session, laps)
}

async fn put(b: &mut Browser<'_>, path: &str, body: Value) -> Value {
    let (st, out) = b.call("PUT", path, body).await;
    assert_eq!(st, StatusCode::OK, "PUT {path}: {out}");
    out
}

/// The numbers of the laps of `owner`'s session that the user sees. Every
/// way to get at a lap must agree: list, detail, download, comparison,
/// telemetry file and the revocation check.
async fn visible(
    b: &mut Browser<'_>,
    owner: (&str, &str),
    session: &Session,
    laps: &[Lap],
) -> Vec<u64> {
    let (owner_id, owner_name) = owner;
    let (st, detail) = b
        .get(&format!("/shared/sessions/{owner_id}/{}", session.id))
        .await;
    let seen: Vec<u64> = match st {
        StatusCode::OK => detail["laps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["number"].as_u64().unwrap())
            .collect(),
        StatusCode::NOT_FOUND => Vec::new(),
        other => panic!("unexpected status {other}: {detail}"),
    };
    let (_, list) = b.get("/shared/sessions").await;
    let listed = list
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == session.id.to_string());
    assert_eq!(listed.is_some(), !seen.is_empty(), "list vs detail");
    if let Some(l) = listed {
        assert_eq!(l["lapCount"], seen.len());
        assert_eq!(l["owner"]["username"], owner_name);
    }
    let (st, download) = b
        .get(&format!(
            "/shared/sessions/{owner_id}/{}/download",
            session.id
        ))
        .await;
    assert_eq!(st == StatusCode::OK, !seen.is_empty(), "download");
    if st == StatusCode::OK {
        assert_eq!(download["laps"].as_array().unwrap().len(), seen.len());
        assert_eq!(download["session"]["setup"], Value::Null);
        assert_eq!(download["session"]["session_info"], Value::Null);
        assert_eq!(download["session"]["origin"]["kind"], "synced");
        assert_eq!(download["session"]["origin"]["owner"], owner_name);
    }
    let check: Vec<Value> = laps
        .iter()
        .map(|l| json!({ "id": l.id, "owner": owner_name }))
        .collect();
    let (st, checked) = b.post("/shared/check", json!({ "laps": check })).await;
    assert_eq!(st, StatusCode::OK, "{checked}");
    for lap in laps {
        let expected = seen.contains(&u64::from(lap.number));
        let (st, _) = b
            .post("/analysis/compare", json!({ "lapIds": [lap.id] }))
            .await;
        assert_eq!(st == StatusCode::OK, expected, "compare lap {}", lap.number);
        let (st, _) = b.get(&format!("/sync/blobs/{}", lap.blob_hash)).await;
        assert_eq!(
            st == StatusCode::OK,
            expected,
            "telemetry lap {}",
            lap.number
        );
        let lost = checked["lost"].as_array().unwrap().contains(&json!(lap.id));
        assert_eq!(lost, !expected, "revocation check lap {}", lap.number);
    }
    seen
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn teams_roles_and_invitations() {
    let Some(s) = server().await else { return };
    let mut admin = Browser::new(&s);
    admin.login(ADMIN, ADMIN_PW).await;
    let (mut anna, anna_id) = user(&s, &mut admin, "anna").await;
    let (mut ben, ben_id) = user(&s, &mut admin, "ben").await;
    let (mut cleo, cleo_id) = user(&s, &mut admin, "cleo").await;

    let (st, team) = anna
        .post("/teams", json!({ "name": "  Sim   Racers " }))
        .await;
    assert_eq!(st, StatusCode::OK, "{team}");
    assert_eq!(team["name"], "Sim Racers");
    assert_eq!(team["role"], "owner");
    let t = team["id"].as_str().unwrap().to_owned();
    assert_eq!(
        anna.post("/teams", json!({ "name": " " })).await.0,
        StatusCode::BAD_REQUEST
    );

    // others don't see the team at all
    assert_eq!(ben.get("/teams").await.1, json!([]));
    assert_eq!(
        ben.get(&format!("/teams/{t}")).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        ben.post(
            &format!("/teams/{t}/invites"),
            json!({ "username": "cleo" })
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        ben.post(&format!("/teams/{t}/leave"), json!({})).await.0,
        StatusCode::NOT_FOUND
    );

    // invite by username: unknown names, twice, accept
    let invites = format!("/teams/{t}/invites");
    assert_eq!(
        anna.post(&invites, json!({ "username": "nobody" })).await.0,
        StatusCode::NOT_FOUND
    );
    let (st, inv) = anna.post(&invites, json!({ "username": "Ben" })).await;
    assert_eq!(st, StatusCode::OK, "{inv}");
    assert_eq!(inv["invitee"]["username"], "ben");
    let (_, again) = anna.post(&invites, json!({ "username": "ben" })).await;
    assert_eq!(again["id"], inv["id"], "one open invitation per user");
    let (_, mine) = ben.get("/me/invites").await;
    assert_eq!(mine.as_array().unwrap().len(), 1);
    assert_eq!(mine[0]["team"]["name"], "Sim Racers");
    assert_eq!(mine[0]["invitedBy"]["username"], "anna");
    let inv_id = mine[0]["id"].as_str().unwrap().to_owned();
    // only the invited user can answer
    assert_eq!(
        cleo.post(&format!("/invites/{inv_id}/accept"), json!({}))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (st, _) = ben
        .post(&format!("/invites/{inv_id}/accept"), json!({}))
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ben.get("/me/invites").await.1, json!([]));
    assert_eq!(
        ben.post(&format!("/invites/{inv_id}/accept"), json!({}))
            .await
            .0,
        StatusCode::NOT_FOUND,
        "single use"
    );
    assert_eq!(
        anna.post(&invites, json!({ "username": "ben" })).await.0,
        StatusCode::CONFLICT,
        "already a member"
    );
    let (_, detail) = ben.get(&format!("/teams/{t}")).await;
    assert_eq!(detail["role"], "member");
    assert_eq!(detail["memberCount"], 2);
    assert_eq!(detail["members"][0]["user"]["username"], "anna");
    assert_eq!(
        detail["invites"],
        json!([]),
        "members don't see invitations"
    );

    // members can't manage
    assert_eq!(
        ben.post(&invites, json!({ "username": "cleo" })).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ben.call("PATCH", &format!("/teams/{t}"), json!({ "name": "X" }))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let member = |id: &str| format!("/teams/{t}/members/{id}");
    assert_eq!(
        ben.call("PATCH", &member(&ben_id), json!({ "role": "admin" }))
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    // a declined invitation, then an invite link
    anna.post(&invites, json!({ "username": "cleo" })).await;
    let (_, mine) = cleo.get("/me/invites").await;
    let id = mine[0]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        cleo.post(&format!("/invites/{id}/decline"), json!({}))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(cleo.get("/teams").await.1, json!([]));
    let (st, link) = anna
        .post(&invites, json!({ "link": { "maxUses": 1 } }))
        .await;
    assert_eq!(st, StatusCode::OK, "{link}");
    let url = link["url"].as_str().unwrap();
    let token = url.rsplit('/').next().unwrap().to_owned();
    assert!(url.starts_with("http://localhost:8080/invite/"), "{url}");
    let (_, detail) = anna.get(&format!("/teams/{t}")).await;
    assert_eq!(detail["invites"].as_array().unwrap().len(), 1);
    assert_eq!(detail["invites"][0]["url"], Value::Null, "shown once");
    let (st, preview) = cleo.get(&format!("/invites/link/{token}")).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(preview["team"]["name"], "Sim Racers");
    assert_eq!(preview["alreadyMember"], false);
    assert_eq!(
        cleo.get("/invites/link/wrong").await.0,
        StatusCode::NOT_FOUND
    );
    // a member following the link doesn't use it up
    let accept = format!("/invites/link/{token}/accept");
    assert_eq!(ben.post(&accept, json!({})).await.0, StatusCode::OK);
    assert_eq!(cleo.post(&accept, json!({})).await.0, StatusCode::OK);
    assert_eq!(cleo.get("/teams").await.1[0]["role"], "member");
    assert_eq!(
        admin.post(&accept, json!({})).await.0,
        StatusCode::NOT_FOUND,
        "used up"
    );
    // a revoked link stops working
    let (_, link) = anna.post(&invites, json!({ "link": {} })).await;
    let token = link["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    let (st, _) = anna
        .call(
            "DELETE",
            &format!("{invites}/{}", link["id"].as_str().unwrap()),
            json!({}),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert_eq!(
        admin
            .post(&format!("/invites/link/{token}/accept"), json!({}))
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    // roles: admins manage members, but not owners
    let (st, _) = anna
        .call("PATCH", &member(&ben_id), json!({ "role": "admin" }))
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert_eq!(
        ben.call("PATCH", &member(&anna_id), json!({ "role": "member" }))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ben.call("PATCH", &member(&cleo_id), json!({ "role": "owner" }))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ben.call("DELETE", &member(&anna_id), json!({})).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ben.call("PATCH", &member(&cleo_id), json!({ "role": "admin" }))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        ben.call("DELETE", &member(&cleo_id), json!({})).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(cleo.get("/teams").await.1, json!([]));
    assert_eq!(
        ben.call("DELETE", &member(&ben_id), json!({})).await.0,
        StatusCode::BAD_REQUEST,
        "leaving is its own action"
    );

    // a team keeps an owner
    assert_eq!(
        anna.call("PATCH", &member(&anna_id), json!({ "role": "member" }))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        anna.post(&format!("/teams/{t}/leave"), json!({})).await.0,
        StatusCode::CONFLICT
    );
    // ownership is handed over, then the first owner can leave
    anna.call("PATCH", &member(&ben_id), json!({ "role": "owner" }))
        .await;
    assert_eq!(
        anna.post(&format!("/teams/{t}/leave"), json!({})).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(anna.get("/teams").await.1, json!([]));
    let (st, renamed) = ben
        .call("PATCH", &format!("/teams/{t}"), json!({ "name": "Ben's" }))
        .await;
    assert_eq!(
        (st, renamed["name"].as_str()),
        (StatusCode::OK, Some("Ben's"))
    );
    // the last member leaving removes the team
    assert_eq!(
        ben.post(&format!("/teams/{t}/leave"), json!({})).await.0,
        StatusCode::NO_CONTENT
    );
    let left: i64 = s
        .state
        .conn()
        .await
        .unwrap()
        .query_one("SELECT count(*) FROM teams", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(left, 0);

    let (_, log) = admin.get("/admin/audit").await;
    let events: Vec<&str> = log
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event"].as_str().unwrap())
        .collect();
    for e in [
        "team_created",
        "team_invite_created",
        "team_joined",
        "team_role_changed",
        "team_member_removed",
        "team_left",
        "team_deleted",
    ] {
        assert!(events.contains(&e), "{e} missing in {events:?}");
    }
}

/// The access-control matrix of docs/ARCHITECTURE.md §10: owner / direct grant /
/// team member / ex-member / everyone (setting on and off) / stranger ×
/// inherit/custom at session and lap level.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sharing_access_matrix() {
    let Some(s) = server().await else { return };
    let mut admin = Browser::new(&s);
    admin.login(ADMIN, ADMIN_PW).await;
    let (mut owner, owner_id) = user(&s, &mut admin, "owner").await;
    let (mut direct, direct_id) = user(&s, &mut admin, "direct").await;
    let (mut mate, _) = user(&s, &mut admin, "mate").await;
    let (mut ex, ex_id) = user(&s, &mut admin, "former").await;
    let (mut stranger, _) = user(&s, &mut admin, "stranger").await;
    let o = (owner_id.as_str(), "owner");

    // a team with owner, mate and ex; ex is removed again
    let (_, team) = owner.post("/teams", json!({ "name": "Team" })).await;
    let t = team["id"].as_str().unwrap().to_owned();
    let (_, link) = owner
        .post(&format!("/teams/{t}/invites"), json!({ "link": {} }))
        .await;
    let token = link["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    let accept = format!("/invites/link/{token}/accept");
    assert_eq!(mate.post(&accept, json!({})).await.0, StatusCode::OK);
    assert_eq!(ex.post(&accept, json!({})).await.0, StatusCode::OK);

    let (session, laps) = push_sample(&mut owner).await;
    let sharing = format!("/sessions/{}/sharing", session.id);
    let lap_sharing = |n: usize| format!("/laps/{}/sharing", laps[n].id);
    let user_grant = json!({ "kind": "user", "id": direct_id });
    let team_grant = json!({ "kind": "team", "id": t });
    let everyone = json!({ "kind": "everyone" });

    macro_rules! expect {
        ($what:literal: direct $d:expr, mate $m:expr, ex $e:expr, stranger $s:expr) => {
            let d: &[u64] = &$d;
            let m: &[u64] = &$m;
            let e: &[u64] = &$e;
            let st: &[u64] = &$s;
            assert_eq!(
                visible(&mut direct, o, &session, &laps).await,
                d,
                "{}: direct",
                $what
            );
            assert_eq!(
                visible(&mut mate, o, &session, &laps).await,
                m,
                "{}: mate",
                $what
            );
            assert_eq!(
                visible(&mut ex, o, &session, &laps).await,
                e,
                "{}: ex",
                $what
            );
            assert_eq!(
                visible(&mut stranger, o, &session, &laps).await,
                st,
                "{}: stranger",
                $what
            );
            // the owner's own data never shows up as shared with them
            assert_eq!(owner.get("/shared/sessions").await.1, json!([]));
            assert_eq!(
                owner.get(&format!("/sessions/{}", session.id)).await.1["laps"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
        };
    }

    // new users share nothing
    let (_, state) = owner.get(&sharing).await;
    assert_eq!(state["mode"], "inherit");
    assert_eq!(state["source"], "default");
    assert_eq!(state["effective"], json!([]));
    expect!("private": direct [], mate [], ex [], stranger []);
    assert_eq!(owner.get("/sessions").await.1[0]["shared"], false);

    // default: one user
    let d = put(
        &mut owner,
        "/me/default-sharing",
        json!({ "grants": [user_grant] }),
    )
    .await;
    assert_eq!(d["inheritingSessions"], 1);
    assert_eq!(d["grants"][0]["username"], "direct");
    expect!("default user": direct [1, 2], mate [], ex [], stranger []);
    assert_eq!(owner.get("/sessions").await.1[0]["shared"], true);
    let (_, state) = owner.get(&lap_sharing(0)).await;
    assert_eq!(state["source"], "default");
    assert_eq!(state["effective"][0]["username"], "direct");

    // default: the team, evaluated live
    put(
        &mut owner,
        "/me/default-sharing",
        json!({ "grants": [team_grant] }),
    )
    .await;
    expect!("default team": direct [], mate [1, 2], ex [1, 2], stranger []);
    let (st, _) = owner
        .call("DELETE", &format!("/teams/{t}/members/{ex_id}"), json!({}))
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    expect!("ex-member": direct [], mate [1, 2], ex [], stranger []);

    // default: everyone, as long as the admin allows it
    put(
        &mut owner,
        "/me/default-sharing",
        json!({ "grants": [everyone] }),
    )
    .await;
    expect!("everyone": direct [1, 2], mate [1, 2], ex [1, 2], stranger [1, 2]);
    let mut settings = admin.get("/admin/settings").await.1;
    settings["allowShareEveryone"] = json!(false);
    put(&mut admin, "/admin/settings", settings.clone()).await;
    expect!("everyone off": direct [], mate [], ex [], stranger []);
    let (_, state) = owner.get(&sharing).await;
    assert_eq!(state["everyoneAllowed"], false);
    assert_eq!(state["effective"], json!([]));
    assert_eq!(owner.get("/sessions").await.1[0]["shared"], false);
    let (st, _) = owner
        .call(
            "PUT",
            &sharing,
            json!({ "mode": "custom", "grants": [everyone] }),
        )
        .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "can't choose everyone while it's off"
    );
    settings["allowShareEveryone"] = json!(true);
    put(&mut admin, "/admin/settings", settings).await;
    expect!("everyone on again": direct [1, 2], mate [1, 2], ex [1, 2], stranger [1, 2]);

    // session custom: its own grants instead of the default
    let state = put(
        &mut owner,
        &sharing,
        json!({ "mode": "custom", "grants": [] }),
    )
    .await;
    assert_eq!(
        (&state["mode"], &state["source"]),
        (&json!("custom"), &json!("own"))
    );
    expect!("session private": direct [], mate [], ex [], stranger []);
    put(
        &mut owner,
        &sharing,
        json!({ "mode": "custom", "grants": [team_grant, team_grant] }),
    )
    .await;
    expect!("session team": direct [], mate [1, 2], ex [], stranger []);
    let (_, state) = owner.get(&lap_sharing(0)).await;
    assert_eq!(state["source"], "session");
    assert_eq!(
        state["effective"],
        json!([{ "kind": "team", "id": t, "name": "Team" }])
    );

    // lap custom: beats the session in both directions
    put(
        &mut owner,
        &lap_sharing(0),
        json!({ "mode": "custom", "grants": [user_grant] }),
    )
    .await;
    expect!("lap 1 for direct": direct [1], mate [2], ex [], stranger []);
    put(
        &mut owner,
        &lap_sharing(1),
        json!({ "mode": "custom", "grants": [] }),
    )
    .await;
    expect!("lap 2 private": direct [1], mate [], ex [], stranger []);
    // a private session with one shared lap
    put(
        &mut owner,
        &sharing,
        json!({ "mode": "custom", "grants": [] }),
    )
    .await;
    put(&mut owner, &lap_sharing(1), json!({ "mode": "inherit" })).await;
    expect!("only lap 1": direct [1], mate [], ex [], stranger []);
    // back to inherit drops the lap's grants
    let state = put(&mut owner, &lap_sharing(0), json!({ "mode": "inherit" })).await;
    assert_eq!(state["grants"], json!([]));
    put(&mut owner, &sharing, json!({ "mode": "inherit" })).await;
    expect!("default again": direct [1, 2], mate [1, 2], ex [1, 2], stranger [1, 2]);

    // leaving the team: the member loses access; what the owner shared
    // with a team stops when the owner leaves
    put(
        &mut owner,
        "/me/default-sharing",
        json!({ "grants": [team_grant] }),
    )
    .await;
    expect!("team": direct [], mate [1, 2], ex [], stranger []);
    assert_eq!(
        ex.post(&accept, json!({})).await.0,
        StatusCode::OK,
        "ex joins again"
    );
    expect!("rejoined": direct [], mate [1, 2], ex [1, 2], stranger []);
    assert_eq!(
        ex.post(&format!("/teams/{t}/leave"), json!({})).await.0,
        StatusCode::NO_CONTENT
    );
    expect!("left": direct [], mate [1, 2], ex [], stranger []);
    let mate_id = mate.get("/me").await.1["id"].as_str().unwrap().to_owned();
    owner
        .call(
            "PATCH",
            &format!("/teams/{t}/members/{mate_id}"),
            json!({ "role": "owner" }),
        )
        .await;
    assert_eq!(
        owner.post(&format!("/teams/{t}/leave"), json!({})).await.0,
        StatusCode::NO_CONTENT
    );
    expect!("owner left the team": direct [], mate [], ex [], stranger []);
    assert_eq!(
        owner.get("/me/default-sharing").await.1["grants"],
        json!([])
    );

    // what can't be shared, and by whom
    for (grants, why) in [
        (json!([team_grant]), "a team the owner isn't in"),
        (json!([{ "kind": "user", "id": owner_id }]), "oneself"),
        (
            json!([{ "kind": "user", "id": uuid::Uuid::new_v4() }]),
            "an unknown user",
        ),
    ] {
        let (st, _) = owner
            .call(
                "PUT",
                &sharing,
                json!({ "mode": "custom", "grants": grants }),
            )
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{why}");
    }
    put(
        &mut owner,
        &sharing,
        json!({ "mode": "custom", "grants": [user_grant] }),
    )
    .await;
    expect!("direct": direct [1, 2], mate [], ex [], stranger []);
    // sharing gives read access only
    assert_eq!(direct.get(&sharing).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        direct
            .call(
                "PUT",
                &sharing,
                json!({ "mode": "custom", "grants": [everyone] })
            )
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        direct
            .call(
                "PUT",
                &lap_sharing(0),
                json!({ "mode": "custom", "grants": [everyone] })
            )
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        direct
            .call("DELETE", &format!("/sessions/{}", session.id), json!({}))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        direct.get(&format!("/sessions/{}", session.id)).await.0,
        StatusCode::NOT_FOUND,
        "not among the viewer's own sessions"
    );
    assert_eq!(
        direct.get("/sync/changes?since=0").await.1["laps"],
        json!([])
    );
    expect!("still direct": direct [1, 2], mate [], ex [], stranger []);

    // finding users: exact name, or people from the same team
    let (st, found) = stranger.get("/users/lookup?username=Owner").await;
    assert_eq!(
        (st, found["id"].as_str()),
        (StatusCode::OK, Some(owner_id.as_str()))
    );
    assert_eq!(
        stranger.get("/users/lookup?username=own").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(stranger.get("/users/suggest?q=").await.1, json!([]));
    assert_eq!(
        owner.get("/users/suggest?q=ma").await.1,
        json!([]),
        "left the team"
    );

    // deleting the session removes its grants
    owner
        .call("DELETE", &format!("/sessions/{}", session.id), json!({}))
        .await;
    assert!(visible(&mut direct, o, &session, &laps).await.is_empty());
    let grants: i64 = s
        .state
        .conn()
        .await
        .unwrap()
        .query_one(
            "SELECT count(*) FROM share_grants WHERE object_kind IN ('session', 'lap')",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(grants, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn imported_sessions_do_not_use_the_default() {
    let Some(s) = server().await else { return };
    let mut admin = Browser::new(&s);
    admin.login(ADMIN, ADMIN_PW).await;
    let (mut owner, owner_id) = user(&s, &mut admin, "owner").await;
    let (mut friend, friend_id) = user(&s, &mut admin, "friend").await;
    let o = (owner_id.as_str(), "owner");

    let (file, session, laps) = bundle();
    let req = owner
        .request("POST", "/import")
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from(file))
        .unwrap();
    assert_eq!(owner.send(req).await.0, StatusCode::OK);
    put(
        &mut owner,
        "/me/default-sharing",
        json!({ "grants": [{ "kind": "everyone" }] }),
    )
    .await;
    let sharing = format!("/sessions/{}/sharing", session.id);
    let (_, state) = owner.get(&sharing).await;
    assert_eq!(state["source"], "imported");
    assert_eq!(state["effective"], json!([]));
    assert_eq!(
        owner.get("/me/default-sharing").await.1["inheritingSessions"],
        0
    );
    assert!(visible(&mut friend, o, &session, &laps).await.is_empty());

    // sharing it explicitly works
    put(
        &mut owner,
        &sharing,
        json!({ "mode": "custom", "grants": [{ "kind": "user", "id": friend_id }] }),
    )
    .await;
    assert_eq!(visible(&mut friend, o, &session, &laps).await, [1, 2]);

    // comparing shared laps works like comparing own ones
    let (st, cmp) = friend
        .post(
            "/analysis/compare",
            json!({ "lapIds": [laps[0].id, laps[1].id] }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{cmp}");
    assert_eq!(cmp["laps"].as_array().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reference_laps_of_team_mates() {
    let Some(s) = server().await else { return };
    let mut admin = Browser::new(&s);
    admin.login(ADMIN, ADMIN_PW).await;
    let (mut ben, ben_id) = user(&s, &mut admin, "ben").await;
    let (mut me, _) = user(&s, &mut admin, "andi").await;
    let (mut stranger, _) = user(&s, &mut admin, "stranger").await;

    let (_, team) = ben.post("/teams", json!({ "name": "Team" })).await;
    let t = team["id"].as_str().unwrap().to_owned();
    let (_, link) = ben
        .post(&format!("/teams/{t}/invites"), json!({ "link": {} }))
        .await;
    let token = link["url"].as_str().unwrap().rsplit('/').next().unwrap();
    let accept = format!("/invites/link/{token}/accept");
    assert_eq!(me.post(&accept, json!({})).await.0, StatusCode::OK);

    // ben shares everything he records with the team; he is faster
    put(
        &mut ben,
        "/me/default-sharing",
        json!({ "grants": [{ "kind": "team", "id": t }] }),
    )
    .await;
    let (_, ben_laps) = push_sample_with(&mut ben, &[9_100, 9_000]).await;
    // my own recording, and an imported file of someone else
    let (_, my_laps) = push_sample(&mut me).await;
    let (file, _, imported_laps) = bundle();
    let req = me
        .request("POST", "/import")
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from(file))
        .unwrap();
    assert_eq!(me.send(req).await.0, StatusCode::OK);

    let (st, tracks) = me.get("/shared/tracks").await;
    assert_eq!(st, StatusCode::OK, "{tracks}");
    assert_eq!(tracks.as_array().unwrap().len(), 1, "{tracks}");
    let track = &tracks[0];
    assert_eq!(track["trackKey"], "iracing:439");
    assert_eq!(track["carKey"], "iracing:67");
    assert_eq!(track["laps"], 2);
    assert_eq!(track["drivers"], 1);
    assert_eq!(track["bestLapMs"], 9_000);
    assert_eq!(track["bestDriver"]["username"], "ben");
    assert_eq!(track["ownBestLapMs"], 9_998);
    assert_eq!(ben.get("/shared/tracks").await.1, json!([]), "not his own");

    let refs =
        |scope: &str| format!("/reference-laps?track=iracing%3A439&car=iracing%3A67&scope={scope}");
    let (st, all) = me.get(&refs("all")).await;
    assert_eq!(st, StatusCode::OK, "{all}");
    let times: Vec<u64> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["lapTimeMs"].as_u64().unwrap())
        .collect();
    assert_eq!(
        times,
        [9_000, 9_100, 9_998, 9_998, 9_999, 9_999],
        "fastest first"
    );
    assert_eq!(all[0]["lapId"], ben_laps[1].id.to_string());
    assert_eq!(all[0]["owner"]["id"], ben_id);
    assert_eq!(all[0]["origin"]["kind"], "synced");
    assert_eq!(all[0]["trackName"], "Winton");
    assert!(all[2]["owner"].is_null());
    let kinds: Vec<&str> = all.as_array().unwrap()[2..]
        .iter()
        .map(|l| l["origin"]["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds.iter().filter(|k| **k == "imported").count(), 2);
    let (_, shared) = me.get(&refs("shared")).await;
    assert_eq!(shared.as_array().unwrap().len(), 2);
    assert!(
        shared
            .as_array()
            .unwrap()
            .iter()
            .all(|l| l["owner"]["username"] == "ben")
    );
    let (_, mine) = me.get(&refs("mine")).await;
    assert_eq!(mine.as_array().unwrap().len(), 4);
    assert!(
        mine.as_array()
            .unwrap()
            .iter()
            .all(|l| l["owner"].is_null())
    );
    let (_, other_car) = me
        .get("/reference-laps?track=iracing%3A439&car=iracing%3A1")
        .await;
    assert_eq!(other_car, json!([]));

    // own lap against a team mate's: the comparison says who drove what
    let (st, cmp) = me
        .post(
            "/analysis/compare",
            json!({ "lapIds": [ben_laps[1].id, my_laps[0].id, imported_laps[0].id] }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{cmp}");
    assert_eq!(cmp["laps"][0]["owner"]["username"], "ben");
    assert_eq!(
        cmp["laps"][0]["driver"], "ben",
        "no display name: the username"
    );
    assert!(cmp["laps"][1]["driver"].is_null());
    assert!(cmp["laps"][1]["owner"].is_null());
    assert_eq!(cmp["laps"][2]["driver"], "Andi");
    assert!(cmp["laps"][2]["owner"].is_null());

    // others see nothing of it
    assert_eq!(stranger.get("/shared/tracks").await.1, json!([]));
    assert_eq!(stranger.get(&refs("all")).await.1, json!([]));
    assert_eq!(
        stranger
            .post("/analysis/compare", json!({ "lapIds": [ben_laps[1].id] }))
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    // leaving the team ends it
    assert_eq!(
        me.post(&format!("/teams/{t}/leave"), json!({})).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(me.get("/shared/tracks").await.1, json!([]));
    assert_eq!(me.get(&refs("shared")).await.1, json!([]));
    assert_eq!(me.get(&refs("all")).await.1.as_array().unwrap().len(), 4);
}

// ----- MCP ------------------------------------------------------------------

/// Posts one JSON-RPC message to `/mcp`.
async fn mcp(
    s: &TestServer,
    token: Option<&str>,
    headers: &[(&str, &str)],
    body: Value,
) -> (StatusCode, Value, axum::http::HeaderMap) {
    let mut b = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream");
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let res = s
        .app
        .clone()
        .oneshot(b.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let (status, headers) = (res.status(), res.headers().clone());
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body, headers)
}

/// A modern (2026-07-28) request with matching headers.
async fn mcp_modern(
    s: &TestServer,
    token: &str,
    id: u32,
    method: &str,
    params: Value,
) -> (StatusCode, Value) {
    let mut params = params;
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": { "name": "test", "version": "1" },
        "io.modelcontextprotocol/clientCapabilities": {}
    });
    let name = params["name"].as_str().map(str::to_owned);
    let mut headers = vec![
        ("mcp-protocol-version", "2026-07-28"),
        ("mcp-method", method),
    ];
    if let Some(n) = &name {
        headers.push(("mcp-name", n.as_str()));
    }
    let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
    let (st, body, _) = mcp(s, Some(token), &headers, body).await;
    (st, body)
}

/// The JSON a tool returned as text.
fn tool_json(result: &Value) -> Value {
    assert_eq!(result["isError"], false, "{result}");
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_endpoint_for_ai_assistants() {
    let Some(s) = server().await else { return };
    let mut b = Browser::new(&s);
    b.login(ADMIN, ADMIN_PW).await;
    let (session, laps) = push_sample(&mut b).await;

    // a read-only token: works for /mcp, nothing else
    let (st, created) = b
        .post("/me/devices", json!({ "name": "Claude", "scope": "read" }))
        .await;
    assert_eq!(st, StatusCode::OK, "{created}");
    assert_eq!(created["device"]["scope"], "read");
    let token = created["token"].as_str().unwrap().to_owned();
    let (st, body) = Browser::device(&s, &token).get("/sessions").await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "read_only_token");
    let (_, devices) = b.get("/me/devices").await;
    assert_eq!(devices[0]["scope"], "read");

    // no or a wrong token: 401 with a Bearer challenge
    let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
    let (st, _, headers) = mcp(&s, None, &[], ping.clone()).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert!(
        headers[header::WWW_AUTHENTICATE]
            .to_str()
            .unwrap()
            .starts_with("Bearer")
    );
    let (st, _, _) = mcp(&s, Some("syd_wrong"), &[], ping.clone()).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    // other websites can't use it
    let (st, _, _) = mcp(
        &s,
        Some(&token),
        &[("origin", "https://evil.example")],
        ping.clone(),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    // no streams, no sessions
    let req = Request::builder()
        .method("GET")
        .uri("/mcp")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        s.app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::METHOD_NOT_ALLOWED
    );

    // legacy clients: initialize, then plain requests
    let init = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": { "name": "test", "version": "1" } } });
    let (st, body, headers) = mcp(&s, Some(&token), &[], init).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(body["result"]["serverInfo"]["name"], "stintory");
    assert!(
        body["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("compare_laps")
    );
    assert!(headers.get("mcp-session-id").is_none(), "stateless");
    let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    let v = [("mcp-protocol-version", "2025-06-18")];
    assert_eq!(
        mcp(&s, Some(&token), &v, note).await.0,
        StatusCode::ACCEPTED
    );
    let list = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
    let (st, body, _) = mcp(&s, Some(&token), &v, list).await;
    assert_eq!(st, StatusCode::OK);
    let tools: Vec<&str> = body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        tools,
        [
            "list_tracks",
            "list_sessions",
            "get_session",
            "best_laps",
            "compare_laps",
            "lap_telemetry"
        ]
    );
    assert!(
        body["result"].get("resultType").is_none(),
        "legacy results stay plain"
    );
    let call = json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": { "name": "list_sessions", "arguments": {} } });
    let (_, body, _) = mcp(&s, Some(&token), &v, call).await;
    let sessions = tool_json(&body["result"]);
    assert_eq!(sessions["total"], 1);
    assert_eq!(sessions["sessions"][0]["sessionId"], session.id.to_string());
    assert_eq!(sessions["sessions"][0]["bestLap"], "0:09.998");

    // modern clients: metadata on every request, mirrored in headers
    let (st, body) = mcp_modern(&s, &token, 4, "server/discover", json!({})).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["resultType"], "complete");
    assert!(
        body["result"]["supportedVersions"]
            .as_array()
            .unwrap()
            .contains(&json!("2026-07-28"))
    );
    assert_eq!(
        body["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "stintory"
    );

    let (_, body) = mcp_modern(
        &s,
        &token,
        5,
        "tools/call",
        json!({ "name": "list_tracks", "arguments": {} }),
    )
    .await;
    let tracks = tool_json(&body["result"]);
    assert_eq!(tracks["driven"][0]["trackKey"], "iracing:439");
    assert_eq!(tracks["driven"][0]["laps"], 2);

    let (_, body) = mcp_modern(
        &s,
        &token,
        6,
        "tools/call",
        json!({ "name": "get_session", "arguments": { "session_id": session.id.to_string() } }),
    )
    .await;
    let detail = tool_json(&body["result"]);
    assert_eq!(detail["laps"].as_array().unwrap().len(), 2);
    assert_eq!(detail["laps"][0]["valid"], true);

    let (_, body) = mcp_modern(
        &s,
        &token,
        7,
        "tools/call",
        json!({ "name": "best_laps",
        "arguments": { "track_key": "iracing:439", "car_key": "iracing:67" } }),
    )
    .await;
    let best = tool_json(&body["result"]);
    assert_eq!(best["laps"][0]["lapId"], laps[1].id.to_string());
    assert_eq!(best["laps"][0]["driver"], "you");

    // lap 1 against lap 2: one corner (both laps cover the distance at the same
    // rate in the sample, so no time is lost; lap 1 is slower at the apex)
    let (_, body) = mcp_modern(&s, &token, 8, "tools/call", json!({ "name": "compare_laps",
        "arguments": { "lap_id": laps[0].id.to_string(), "reference_lap_id": laps[1].id.to_string() } })).await;
    let cmp = tool_json(&body["result"]);
    assert_eq!(cmp["corners"].as_array().unwrap().len(), 1, "{cmp}");
    assert_eq!(cmp["corners"][0]["direction"], "right");
    assert_eq!(cmp["corners"][0]["timeLostS"], 0.0, "{cmp}");
    assert!(
        cmp["corners"][0]["lap"]["minSpeedKmh"].as_f64().unwrap()
            < cmp["corners"][0]["reference"]["minSpeedKmh"]
                .as_f64()
                .unwrap()
    );
    assert_eq!(cmp["lap"]["number"], 1);

    let (_, body) = mcp_modern(
        &s,
        &token,
        9,
        "tools/call",
        json!({ "name": "lap_telemetry",
        "arguments": { "lap_id": laps[0].id.to_string(), "reference_lap_id": laps[1].id.to_string(),
                       "from_m": 300, "to_m": 400, "step_m": 10, "channels": ["speed", "nope"] } }),
    )
    .await;
    let tel = tool_json(&body["result"]);
    assert_eq!(tel["distM"].as_array().unwrap().len(), 11);
    assert_eq!(tel["units"]["speed"], "km/h");
    assert_eq!(tel["lap"]["channels"]["speed"][0], 183.6, "51 m/s");
    assert!(tel["lap"]["deltaS"].is_array());
    assert_eq!(tel["unknownChannels"], json!(["nope"]));

    // tool errors are results the model can correct
    let (st, body) = mcp_modern(
        &s,
        &token,
        10,
        "tools/call",
        json!({ "name": "get_session", "arguments": { "session_id": "nope" } }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["result"]["isError"], true);
    let (_, body) = mcp_modern(
        &s,
        &token,
        11,
        "tools/call",
        json!({ "name": "get_session",
        "arguments": { "session_id": uuid::Uuid::new_v4().to_string() } }),
    )
    .await;
    assert_eq!(
        body["result"]["isError"], true,
        "someone else's or no session"
    );

    // protocol errors
    let (st, body) = mcp_modern(
        &s,
        &token,
        12,
        "tools/call",
        json!({ "name": "delete_everything" }),
    )
    .await;
    assert_eq!(
        (st, body["error"]["code"].clone()),
        (StatusCode::BAD_REQUEST, json!(-32602))
    );
    let (st, body) = mcp_modern(&s, &token, 13, "resources/list", json!({})).await;
    assert_eq!(
        (st, body["error"]["code"].clone()),
        (StatusCode::NOT_FOUND, json!(-32601))
    );
    let meta = json!({ "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                       "io.modelcontextprotocol/clientCapabilities": {} });
    let call = json!({ "jsonrpc": "2.0", "id": 14, "method": "tools/call",
        "params": { "name": "list_tracks", "_meta": meta } });
    let wrong = [
        ("mcp-protocol-version", "2026-07-28"),
        ("mcp-method", "tools/call"),
        ("mcp-name", "list_sessions"),
    ];
    let (st, body, _) = mcp(&s, Some(&token), &wrong, call).await;
    assert_eq!(
        (st, body["error"]["code"].clone()),
        (StatusCode::BAD_REQUEST, json!(-32020))
    );
    let old = json!({ "jsonrpc": "2.0", "id": 15, "method": "tools/list",
        "params": { "_meta": { "io.modelcontextprotocol/protocolVersion": "2099-01-01",
                               "io.modelcontextprotocol/clientCapabilities": {} } } });
    let (st, body, _) = mcp(
        &s,
        Some(&token),
        &[
            ("mcp-protocol-version", "2099-01-01"),
            ("mcp-method", "tools/list"),
        ],
        old,
    )
    .await;
    assert_eq!(
        (st, body["error"]["code"].clone()),
        (StatusCode::BAD_REQUEST, json!(-32022))
    );
    assert_eq!(body["error"]["data"]["requested"], "2099-01-01");

    // revoking the token ends the access
    let id = created["device"]["id"].as_str().unwrap();
    assert_eq!(
        b.call("DELETE", &format!("/me/devices/{id}"), json!({}))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        mcp(&s, Some(&token), &[], ping).await.0,
        StatusCode::UNAUTHORIZED
    );
}

// ----- OAuth for MCP clients -------------------------------------------------

async fn raw(s: &TestServer, req: Request<Body>) -> (StatusCode, Value, axum::http::HeaderMap) {
    let res = s.app.clone().oneshot(req).await.unwrap();
    let (status, headers) = (res.status(), res.headers().clone());
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        headers,
    )
}

/// `POST /oauth/token` with a form body.
async fn token_request(s: &TestServer, form: &[(&str, &str)]) -> (StatusCode, Value) {
    let body = form
        .iter()
        .map(|(k, v)| format!("{k}={}", url_enc(v)))
        .collect::<Vec<_>>()
        .join("&");
    let req = Request::builder()
        .method("POST")
        .uri("/oauth/token")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    let (st, body, headers) = raw(s, req).await;
    if st == StatusCode::OK {
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    }
    (st, body)
}

fn url_enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A query parameter of a redirect URL.
fn redirect_param(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == name).then(|| {
            let mut out = Vec::new();
            let b = v.as_bytes();
            let mut i = 0;
            while i < b.len() {
                if b[i] == b'%' {
                    out.push(
                        u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap(), 16)
                            .unwrap(),
                    );
                    i += 3;
                } else {
                    out.push(b[i]);
                    i += 1;
                }
            }
            String::from_utf8(out).unwrap()
        })
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oauth_for_mcp_clients() {
    let Some(s) = server().await else { return };
    let base = "http://localhost:8080";
    let get = |path: &str| Request::builder().uri(path).body(Body::empty()).unwrap();

    // discovery: 401 → protected resource metadata → authorization server
    let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
    let (st, _, headers) = mcp(&s, None, &[], ping.clone()).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let challenge = headers[header::WWW_AUTHENTICATE]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        challenge.contains(&format!(
            r#"resource_metadata="{base}/.well-known/oauth-protected-resource/mcp""#
        )),
        "{challenge}"
    );
    assert!(challenge.contains(r#"scope="laps:read""#));
    let (st, prm, _) = raw(&s, get("/.well-known/oauth-protected-resource/mcp")).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(prm["resource"], format!("{base}/mcp"));
    assert_eq!(prm["authorization_servers"], json!([base]));
    assert_eq!(
        raw(&s, get("/.well-known/oauth-protected-resource"))
            .await
            .1,
        prm
    );
    let (_, asm, _) = raw(&s, get("/.well-known/oauth-authorization-server")).await;
    assert_eq!(asm["issuer"], base);
    assert_eq!(asm["code_challenge_methods_supported"], json!(["S256"]));
    assert_eq!(asm["client_id_metadata_document_supported"], true);
    assert_eq!(asm["authorization_response_iss_parameter_supported"], true);
    assert_eq!(asm["token_endpoint"], format!("{base}/oauth/token"));

    // dynamic registration (public clients, https or loopback redirects)
    let register = |body: Value| {
        Request::builder()
            .method("POST")
            .uri("/oauth/register")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let (st, client, _) = raw(
        &s,
        register(json!({
        "client_name": "Test Assistant", "redirect_uris": ["http://127.0.0.1:9999/callback"],
        "token_endpoint_auth_method": "none" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{client}");
    assert_eq!(client["token_endpoint_auth_method"], "none");
    let client_id = client["client_id"].as_str().unwrap().to_owned();
    let (st, err, _) = raw(
        &s,
        register(json!({ "redirect_uris": ["http://evil.example/cb"] })),
    )
    .await;
    assert_eq!(
        (st, err["error"].clone()),
        (StatusCode::BAD_REQUEST, json!("invalid_redirect_uri"))
    );

    // the user logs in and looks at the request (loopback: another port is fine)
    let mut b = Browser::new(&s);
    b.login(ADMIN, ADMIN_PW).await;
    push_sample(&mut b).await;
    let (verifier, challenge) = pkce();
    let redirect = "http://127.0.0.1:5555/callback";
    let query = |extra: &str| {
        format!(
            "?response_type=code&client_id={}&redirect_uri={}&code_challenge={challenge}\
             &code_challenge_method=S256&state=xyz&scope=laps%3Aread&resource={}{extra}",
            url_enc(&client_id),
            url_enc(redirect),
            url_enc(&format!("{base}/mcp"))
        )
    };
    let (st, info) = b.get(&format!("/oauth/authorize{}", query(""))).await;
    assert_eq!(st, StatusCode::OK, "{info}");
    assert_eq!(info["clientName"], "Test Assistant");
    assert_eq!(info["redirectHost"], "127.0.0.1");
    assert_eq!(info["localRedirect"], true);
    assert_eq!(info["clientHost"], Value::Null, "registered, not verified");
    // an unregistered redirect is shown, never followed
    let bad = format!(
        "/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&code_challenge={challenge}&code_challenge_method=S256",
        url_enc(&client_id),
        url_enc("https://evil.example/cb")
    );
    assert_eq!(b.get(&bad).await.0, StatusCode::BAD_REQUEST);
    // other problems go back to the client, with iss
    let no_pkce = format!(
        "/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&state=s1",
        url_enc(&client_id),
        url_enc(redirect)
    );
    let (_, info) = b.get(&no_pkce).await;
    let to = info["redirectTo"].as_str().unwrap();
    assert_eq!(
        redirect_param(to, "error").as_deref(),
        Some("invalid_request")
    );
    assert_eq!(redirect_param(to, "state").as_deref(), Some("s1"));
    assert_eq!(redirect_param(to, "iss").as_deref(), Some(base));

    let params = json!({
        "response_type": "code", "client_id": client_id, "redirect_uri": redirect,
        "code_challenge": challenge, "code_challenge_method": "S256", "state": "xyz",
        "resource": format!("{base}/mcp"),
    });
    // denied
    let (_, denied) = b
        .post(
            "/oauth/approve",
            json!({ "params": params, "allow": false }),
        )
        .await;
    let to = denied["redirectTo"].as_str().unwrap();
    assert_eq!(
        redirect_param(to, "error").as_deref(),
        Some("access_denied")
    );
    assert!(redirect_param(to, "code").is_none());
    // allowed
    let (st, approved) = b
        .post("/oauth/approve", json!({ "params": params, "allow": true }))
        .await;
    assert_eq!(st, StatusCode::OK, "{approved}");
    let to = approved["redirectTo"].as_str().unwrap();
    assert!(to.starts_with(redirect), "{to}");
    assert_eq!(redirect_param(to, "state").as_deref(), Some("xyz"));
    assert_eq!(redirect_param(to, "iss").as_deref(), Some(base));
    let code = redirect_param(to, "code").unwrap();

    // the code: a wrong verifier uses it up; a new approval, then the right one (once)
    let exchange = |code: &str, verifier: &str| {
        vec![
            ("grant_type", "authorization_code".to_owned()),
            ("code", code.to_owned()),
            ("client_id", client_id.clone()),
            ("redirect_uri", redirect.to_owned()),
            ("code_verifier", verifier.to_owned()),
            ("resource", format!("{base}/mcp")),
        ]
    };
    let send = |v: Vec<(&'static str, String)>| {
        let s = &s;
        async move {
            let pairs: Vec<(&str, &str)> = v.iter().map(|(k, v)| (*k, v.as_str())).collect();
            token_request(s, &pairs).await
        }
    };
    let (wrong_verifier, _) = pkce();
    let (st, err) = send(exchange(&code, &wrong_verifier)).await;
    assert_eq!(
        (st, err["error"].clone()),
        (StatusCode::BAD_REQUEST, json!("invalid_grant"))
    );
    let (_, approved) = b
        .post("/oauth/approve", json!({ "params": params, "allow": true }))
        .await;
    let code = redirect_param(approved["redirectTo"].as_str().unwrap(), "code").unwrap();
    let exchange = exchange(&code, &verifier);
    let (st, tokens) = send(exchange.clone()).await;
    assert_eq!(st, StatusCode::OK, "{tokens}");
    assert_eq!(tokens["token_type"], "Bearer");
    assert_eq!(tokens["expires_in"], 3600);
    let access = tokens["access_token"].as_str().unwrap().to_owned();
    let refresh = tokens["refresh_token"].as_str().unwrap().to_owned();
    assert!(access.starts_with("syo_") && refresh.starts_with("syr_"));
    assert_eq!(
        send(exchange).await.1["error"],
        "invalid_grant",
        "single use"
    );

    // the access token reads through MCP, and nothing else
    let list = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "list_sessions", "arguments": {} } });
    let (st, body, _) = mcp(&s, Some(&access), &[], list.clone()).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(tool_json(&body["result"])["total"], 1);
    assert_eq!(
        Browser::device(&s, &access).get("/sessions").await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        mcp(&s, Some(&refresh), &[], ping.clone()).await.0,
        StatusCode::UNAUTHORIZED
    );

    // refresh rotates: the old refresh token stops working
    let (st, renewed) = token_request(
        &s,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh),
            ("client_id", &client_id),
        ],
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{renewed}");
    let access2 = renewed["access_token"].as_str().unwrap().to_owned();
    let refresh2 = renewed["refresh_token"].as_str().unwrap().to_owned();
    assert_ne!(refresh2, refresh);
    let (st, err) = token_request(
        &s,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh),
            ("client_id", &client_id),
        ],
    )
    .await;
    assert_eq!(
        (st, err["error"].clone()),
        (StatusCode::BAD_REQUEST, json!("invalid_grant"))
    );
    assert_eq!(
        mcp(&s, Some(&access2), &[], ping.clone()).await.0,
        StatusCode::OK
    );

    // the connection is on the Account page; removing it ends the access
    let (_, devices) = b.get("/me/devices").await;
    let grant = devices
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "Test Assistant")
        .unwrap()
        .clone();
    assert_eq!(grant["scope"], "read");
    let id = grant["id"].as_str().unwrap();
    assert_eq!(
        b.call("DELETE", &format!("/me/devices/{id}"), json!({}))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        mcp(&s, Some(&access2), &[], ping).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (_, err) = token_request(
        &s,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh2),
            ("client_id", &client_id),
        ],
    )
    .await;
    assert_eq!(err["error"], "invalid_grant");

    // a client known from its metadata document (cached, so no fetch here)
    let doc_id = "https://assistant.example/oauth/client.json";
    s.state
        .conn()
        .await
        .unwrap()
        .execute(
            "INSERT INTO oauth_clients (client_id, client_name, redirect_uris, source, fetched_at)
             VALUES ($1, 'Example Assistant', $2, 'metadata_document', now())",
            &[&doc_id, &json!(["https://assistant.example/callback"])],
        )
        .await
        .unwrap();
    let doc_query = format!(
        "/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&code_challenge={challenge}&code_challenge_method=S256",
        url_enc(doc_id),
        url_enc("https://assistant.example/callback")
    );
    let (st, info) = b.get(&doc_query).await;
    assert_eq!(st, StatusCode::OK, "{info}");
    assert_eq!(info["clientHost"], "assistant.example");
    assert_eq!(info["localRedirect"], false);
    // tokens only for this server's MCP endpoint
    let (_, info) = b
        .get(&format!(
            "{doc_query}&resource={}",
            url_enc("https://other.example/mcp")
        ))
        .await;
    let to = info["redirectTo"].as_str().unwrap();
    assert_eq!(
        redirect_param(to, "error").as_deref(),
        Some("invalid_target")
    );
}

/// A server without internet access: clients register themselves instead
/// of naming a metadata document.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oauth_without_client_metadata_documents() {
    let Some(s) = server_with(|c| c.oauth_client_metadata = false).await else {
        return;
    };
    let req = Request::builder()
        .uri("/.well-known/oauth-authorization-server")
        .body(Body::empty())
        .unwrap();
    let (_, asm, _) = raw(&s, req).await;
    assert_eq!(asm["client_id_metadata_document_supported"], false);
    assert!(asm["registration_endpoint"].is_string());

    let mut b = Browser::new(&s);
    b.login(ADMIN, ADMIN_PW).await;
    let (_, challenge) = pkce();
    let authorize = |client: &str, redirect: &str| {
        format!(
            "/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&code_challenge={challenge}&code_challenge_method=S256",
            url_enc(client),
            url_enc(redirect)
        )
    };
    // not fetched: the message says what to do
    let doc = "https://claude.ai/oauth/claude-code-client-metadata";
    let (st, body) = b
        .get(&authorize(doc, "http://localhost:4000/callback"))
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(
        body["error"].as_str().unwrap().contains("registers itself"),
        "{body}"
    );
    // a document fetched earlier still works
    s.state
        .conn()
        .await
        .unwrap()
        .execute(
            "INSERT INTO oauth_clients (client_id, client_name, redirect_uris, source, fetched_at)
             VALUES ($1, 'Old', $2, 'metadata_document', now() - interval '30 days')",
            &[&doc, &json!(["http://localhost/callback"])],
        )
        .await
        .unwrap();
    let (st, info) = b
        .get(&authorize(doc, "http://localhost:4000/callback"))
        .await;
    assert_eq!(st, StatusCode::OK, "{info}");
    assert_eq!(info["clientName"], "Old");
}

/// A device token for `b`'s user, created on the account page.
async fn device_token(b: &mut Browser<'_>) -> String {
    let (st, body) = b.post("/me/devices", json!({ "name": "pc" })).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    body["token"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn password_changes_and_resets_disconnect_devices() {
    let Some(s) = server().await else { return };
    let mut admin = Browser::new(&s);
    admin.login(ADMIN, ADMIN_PW).await;
    let (mut u, id) = user(&s, &mut admin, "racer").await;
    let connected = |token: &str| {
        let token = token.to_owned();
        let s = &s;
        async move { Browser::device(s, &token).get("/devices/current").await.0 == StatusCode::OK }
    };

    // a routine change keeps the devices
    let token = device_token(&mut u).await;
    let (st, _) = u
        .post(
            "/me/password",
            json!({ "currentPassword": "a long password 1", "newPassword": "a long password 2" }),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(connected(&token).await);

    // on request it disconnects them
    let (st, _) = u
        .post(
            "/me/password",
            json!({
                "currentPassword": "a long password 2",
                "newPassword": "a long password 3",
                "disconnectDevices": true
            }),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(!connected(&token).await);
    assert_eq!(u.get("/me/devices").await.1, json!([]));
    assert!(u.get("/me").await.0 == StatusCode::OK, "own session stays");

    // an admin reset always does
    let token = device_token(&mut u).await;
    let (st, _) = admin
        .call(
            "PATCH",
            &format!("/admin/users/{id}"),
            json!({ "resetPassword": true }),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(!connected(&token).await);
}

#[tokio::test]
async fn password_confirmations_are_rate_limited() {
    let Some(s) = server().await else { return };
    let mut admin = Browser::new(&s);
    admin.login(ADMIN, ADMIN_PW).await;
    let (mut u, _) = user(&s, &mut admin, "racer").await;
    for _ in 0..sy_server::ratelimit::PER_ACCOUNT {
        let (st, body) = u
            .call(
                "DELETE",
                "/me/totp",
                json!({ "password": "wrong password!" }),
            )
            .await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    }
    // further guesses are refused, even the right password
    let (st, _) = u
        .call(
            "DELETE",
            "/me/totp",
            json!({ "password": "a long password 1" }),
        )
        .await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS);
    let (st, _) = u
        .post(
            "/me/passkeys/register/start",
            json!({ "password": "a long password 1" }),
        )
        .await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn blob_uploads_are_limited_and_unused_files_removed() {
    let Some(s) = server_with(|c| {
        c.max_blob_mb = 1;
        c.max_upload_mb = 1;
    })
    .await
    else {
        return;
    };
    let mut b = Browser::new(&s);
    b.login(ADMIN, ADMIN_PW).await;

    // larger than STINTORY_MAX_BLOB_MB / STINTORY_MAX_UPLOAD_MB
    let big = vec![7u8; 2 << 20];
    let req = b
        .request("PUT", &format!("/sync/blobs/{}", BlobHash::of(&big)))
        .body(Body::from(big.clone()))
        .unwrap();
    assert_eq!(b.send(req).await.0, StatusCode::PAYLOAD_TOO_LARGE);
    let req = b.request("POST", "/import").body(Body::from(big)).unwrap();
    assert_eq!(b.send(req).await.0, StatusCode::PAYLOAD_TOO_LARGE);

    // both files uploaded, only one lap pushed; plus a file without a row
    let (session, laps) = sample();
    for (lap, bytes) in &laps {
        let req = b
            .request("PUT", &format!("/sync/blobs/{}", lap.blob_hash))
            .body(Body::from(bytes.clone()))
            .unwrap();
        assert_eq!(b.send(req).await.0, StatusCode::NO_CONTENT);
    }
    let (st, body) = b
        .post(
            "/sync/push",
            json!({ "sessions": [session], "laps": [laps[0].0] }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let stray = s.state.blobs.put(b"left over by a failed import").unwrap();
    let (used, unused) = (&laps[0].0.blob_hash, &laps[1].0.blob_hash);

    // young files stay: their laps may still come
    let hour = std::time::Duration::from_secs(3600);
    assert_eq!(
        sy_server::sync::remove_orphan_blobs(&s.state, hour)
            .await
            .unwrap(),
        0
    );
    assert!(s.state.blobs.contains(unused) && s.state.blobs.contains(&stray));

    // old ones go, the used one stays
    assert_eq!(
        sy_server::sync::remove_orphan_blobs(&s.state, std::time::Duration::ZERO)
            .await
            .unwrap(),
        2
    );
    assert!(s.state.blobs.contains(used));
    assert!(!s.state.blobs.contains(unused) && !s.state.blobs.contains(&stray));
    let (_, body) = b
        .post(
            "/sync/blobs/missing",
            json!({ "hashes": [used.to_string(), unused.to_string()] }),
        )
        .await;
    assert_eq!(body["missing"], json!([unused.to_string()]));

    // a cleanup while files are being stored skips its run
    let writing = s.state.blob_writes.read().await;
    s.state.blobs.put(b"another stray").unwrap();
    assert_eq!(
        sy_server::sync::remove_orphan_blobs(&s.state, std::time::Duration::ZERO)
            .await
            .unwrap(),
        0
    );
    drop(writing);
}

#[tokio::test]
async fn expired_rows_are_removed() {
    let Some(s) = server().await else { return };
    let mut old = Browser::new(&s);
    old.login(ADMIN, ADMIN_PW).await;
    let mut current = Browser::new(&s);
    current.login(ADMIN, ADMIN_PW).await;
    let db = s.state.conn().await.unwrap();
    // the first session expired, the second is valid
    db.execute(
        "UPDATE web_sessions SET expires_at = now() - interval '1 minute'
         WHERE created_at = (SELECT min(created_at) FROM web_sessions)",
        &[],
    )
    .await
    .unwrap();
    let count = |sql: &'static str| {
        let db = &db;
        async move { db.query_one(sql, &[]).await.unwrap().get::<_, i64>(0) }
    };
    assert_eq!(count("SELECT count(*) FROM web_sessions").await, 2);
    assert!(
        sy_server::cleanup::remove_expired(&db).await.unwrap() >= 1,
        "the expired session"
    );
    assert_eq!(count("SELECT count(*) FROM web_sessions").await, 1);
    assert_eq!(current.get("/me").await.0, StatusCode::OK);
    assert_eq!(old.get("/me").await.0, StatusCode::UNAUTHORIZED);
    sy_server::cleanup::run_once(&s.state).await;
}
