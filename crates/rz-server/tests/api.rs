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
}

impl<'a> Browser<'a> {
    fn new(server: &'a TestServer) -> Self {
        Self {
            server,
            cookie: None,
            csrf: None,
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

fn bundle() -> (Vec<u8>, Session, Vec<Lap>) {
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
    let mut w = RzxWriter::new(Cursor::new(Vec::new()));
    w.add_session(&session).unwrap();
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
        w.add_lap(&lap, &bytes).unwrap();
        laps.push(lap);
    }
    let file = w.finish("test", Some("Andi")).unwrap().into_inner();
    (file, session, laps)
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
