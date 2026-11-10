// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! API tests against a real Postgres. Set `RZ_TEST_DATABASE_URL` (see
//! docker/compose.dev.yml); without it the tests are skipped. Every test
//! runs in its own schema.

use std::io::Cursor;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use rz_model::channel::names;
use rz_model::rzx::RzxWriter;
use rz_model::session::{Conditions, LapFlags};
use rz_model::{
    BlobHash, Channel, ChannelData, GameId, Lap, LapId, LapTelemetry, Origin, Session, SessionId,
    SessionType, Unit,
};
use rz_server::config::Config;
use rz_server::crypto;
use rz_server::state::{Registration, ServerSettings, SharedState};
use serde_json::{Value, json};
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
    let Ok(url) = std::env::var("RZ_TEST_DATABASE_URL") else {
        eprintln!("RZ_TEST_DATABASE_URL not set, skipped");
        return None;
    };
    let schema = format!("t_{}", uuid::Uuid::new_v4().simple());
    let (client, conn) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
        .await
        .expect("connect to RZ_TEST_DATABASE_URL");
    tokio::spawn(conn);
    client
        .batch_execute(&format!("CREATE SCHEMA {schema}"))
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::for_tests(url.clone(), dir.path().to_path_buf());
    config.admin_username = Some(ADMIN.into());
    config.admin_password = Some(ADMIN_PW.into());
    let state = rz_server::init(config, Some(&schema)).await.unwrap();
    Some(TestServer {
        app: rz_server::app(state.clone()),
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
    for _ in 0..rz_server::ratelimit::PER_ACCOUNT {
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

/// `sample()` as an .rzx file.
fn bundle() -> (Vec<u8>, Session, Vec<Lap>) {
    let (session, laps) = sample();
    let mut w = RzxWriter::new(Cursor::new(Vec::new()));
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
    assert!(token.starts_with("rzd_"));
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
