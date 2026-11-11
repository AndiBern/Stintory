// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! End to end: a real sy-server, the browser-based device login, and two
//! local stores syncing through the server. Needs `STINTORY_TEST_DATABASE_URL`
//! (see docker/compose.dev.yml); skipped without it.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use sy_capture::pipeline::SharedStore;
use sy_model::channel::names;
use sy_model::session::Conditions;
use sy_model::{
    Channel, ChannelData, GameId, LapFlags, LapId, LapTelemetry, NewLap, Origin, Session,
    SessionId, SessionType, Unit,
};
use sy_server::config::Config;
use sy_store::Store;
use sy_sync::{Client, DeviceLogin, SyncError, sync_once};

const ADMIN_PW: &str = "admin password 123";

struct Server {
    url: String,
    db_url: String,
    schema: String,
    _dir: tempfile::TempDir,
}

impl Drop for Server {
    fn drop(&mut self) {
        let (url, schema) = (self.db_url.clone(), self.schema.clone());
        let _ = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    if let Ok((c, conn)) =
                        tokio_postgres::connect(&url, tokio_postgres::NoTls).await
                    {
                        tokio::spawn(conn);
                        let _ = c
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

async fn server() -> Option<Server> {
    let Ok(db_url) = std::env::var("STINTORY_TEST_DATABASE_URL") else {
        eprintln!("STINTORY_TEST_DATABASE_URL not set, skipped");
        return None;
    };
    let schema = format!("t_{}", uuid::Uuid::new_v4().simple());
    let (c, conn) = tokio_postgres::connect(&db_url, tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(conn);
    c.batch_execute(&format!("CREATE SCHEMA {schema}"))
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let mut config = Config::for_tests(db_url.clone(), dir.path().to_path_buf());
    config.public_url = url.clone();
    config.admin_username = Some("admin".into());
    config.admin_password = Some(ADMIN_PW.into());
    let state = sy_server::init(config, Some(&schema)).await.unwrap();
    tokio::spawn(axum::serve(listener, sy_server::app(state)).into_future());
    Some(Server {
        url,
        db_url,
        schema,
        _dir: dir,
    })
}

/// The browser part of the device login: log in, approve, follow the
/// redirect back to the app.
async fn approve_in_browser(server: &Server, login_url: &str) {
    let browser = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    let res: Value = browser
        .post(format!("{}/api/v1/auth/login", server.url))
        .json(&json!({ "username": "admin", "password": ADMIN_PW }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let csrf = res["csrfToken"].as_str().unwrap().to_owned();
    let query = login_url.split_once('?').unwrap().1;
    let param = |n: &str| {
        let v = query
            .split('&')
            .find_map(|p| p.strip_prefix(&format!("{n}=")))
            .unwrap();
        // decode %XX
        let mut out = String::new();
        let b = v.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' {
                out.push(u8::from_str_radix(&v[i + 1..i + 3], 16).unwrap() as char);
                i += 3;
            } else {
                out.push(b[i] as char);
                i += 1;
            }
        }
        out
    };
    let approve: Value = browser
        .post(format!("{}/api/v1/device-login/approve", server.url))
        .header("x-csrf-token", csrf)
        .json(&json!({
            "redirectUri": param("redirect_uri"),
            "codeChallenge": param("code_challenge"),
            "state": param("state"),
            "deviceName": param("device_name"),
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let back = browser
        .get(approve["redirectTo"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(back.status(), 200);
    assert!(back.text().await.unwrap().contains("connected"));
}

async fn connect(server: &Server, name: &str) -> Client {
    let client = Client::new(&server.url).unwrap();
    let (login, url) = DeviceLogin::start(client.clone(), name).await.unwrap();
    assert!(url.starts_with(&format!("{}/device-login?", server.url)));
    let waiting = tokio::spawn(login.finish(Duration::from_secs(30)));
    approve_in_browser(server, &url).await;
    let device = waiting.await.unwrap().unwrap();
    assert_eq!(device.device.name, name);
    client.with_token(device.token)
}

fn store(dir: &std::path::Path) -> SharedStore {
    SharedStore(Arc::new(Mutex::new(Store::open(dir).unwrap())))
}

/// Records a session with `laps` laps into `store`, started `hours_ago`.
fn record(store: &SharedStore, laps: u32, hours_ago: i64) -> SessionId {
    let s = Session {
        id: SessionId::new(),
        game: GameId::Iracing,
        game_version: None,
        track_key: "iracing:439".into(),
        track_name: "Winton".into(),
        car_key: "iracing:67".into(),
        car_name: "MX-5".into(),
        session_type: SessionType::Practice,
        started_at: time::OffsetDateTime::now_utc() - time::Duration::hours(hours_ago),
        ended_at: None,
        conditions: Conditions::default(),
        setup: None,
        session_info: None,
        origin: Origin::Own,
    };
    store.with(|st| {
        st.insert_session(&s).unwrap();
        for n in 1..=laps {
            let t = LapTelemetry::new(60.0)
                .with_channel(Channel::new(
                    names::SPEED,
                    Unit::MeterPerSecond,
                    ChannelData::F32(vec![40.0 + n as f32; 600]),
                ))
                .unwrap();
            st.insert_lap(
                NewLap {
                    id: LapId::new(),
                    session_id: s.id,
                    number: n,
                    start_time_s: Some(f64::from(n) * 95.0),
                    lap_time_ms: Some(95_000 + n),
                    sector_times_ms: vec![],
                    valid: true,
                    invalid_reason: None,
                    flags: LapFlags::default(),
                },
                &t,
            )
            .unwrap();
        }
    });
    s.id
}

fn summary(store: &SharedStore) -> Vec<(SessionId, u32, Option<u32>)> {
    store.with(|s| {
        s.list_sessions()
            .unwrap()
            .into_iter()
            .map(|x| (x.id, x.lap_count, x.best_lap_ms))
            .collect()
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn record_on_one_device_view_on_another() {
    let Some(server) = server().await else { return };
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (windows, linux) = (store(da.path()), store(db.path()));
    let wc = connect(&server, "Windows PC").await;
    let lc = connect(&server, "Linux laptop").await;
    assert_eq!(wc.current_device().await.unwrap().user.username, "admin");

    // recorded on Windows → appears on Linux
    let s1 = record(&windows, 3, 0);
    let r = sync_once(&wc, &windows).await.unwrap();
    assert_eq!(
        (r.pushed_sessions, r.pushed_laps, r.uploaded_files),
        (1, 3, 3)
    );
    let r = sync_once(&lc, &linux).await.unwrap();
    assert_eq!((r.pulled_sessions, r.pulled_laps), (1, 3));
    assert_eq!(summary(&linux), summary(&windows));
    let lap = windows.with(|s| s.laps(s1).unwrap())[0].clone();
    assert_eq!(
        linux.with(|s| s.lap_telemetry(lap.id).unwrap()),
        windows.with(|s| s.lap_telemetry(lap.id).unwrap())
    );
    assert_eq!(linux.with(|s| s.session(s1).unwrap().origin), Origin::Own);

    // nothing to do the second time, nothing bounces back
    let r = sync_once(&lc, &linux).await.unwrap();
    assert_eq!((r.pushed_sessions, r.pushed_laps, r.pulled_laps), (0, 0, 0));
    let r = sync_once(&wc, &windows).await.unwrap();
    assert_eq!((r.pushed_sessions, r.pushed_laps), (0, 0));

    // more laps of the running session, and an imported session on Linux
    windows.with(|s| {
        let mut session = s.session(s1).unwrap();
        session.ended_at = Some(time::OffsetDateTime::now_utc());
        s.update_session(&session).unwrap();
    });
    let s2 = record(&linux, 2, 1);
    sync_once(&wc, &windows).await.unwrap();
    sync_once(&lc, &linux).await.unwrap();
    sync_once(&wc, &windows).await.unwrap();
    assert_eq!(summary(&windows), summary(&linux));
    assert_eq!(summary(&windows).len(), 2);
    assert!(linux.with(|s| s.session(s1).unwrap().ended_at.is_some()));

    // deleting on one device deletes everywhere
    windows.with(|s| s.delete_session(s2).unwrap());
    sync_once(&wc, &windows).await.unwrap();
    let r = sync_once(&lc, &linux).await.unwrap();
    assert_eq!(r.pulled_deletions, 1);
    assert_eq!(summary(&linux).len(), 1);
    assert_eq!(summary(&windows), summary(&linux));

    // disconnecting revokes the token; the device can't sync anymore
    lc.disconnect().await.unwrap();
    assert!(matches!(
        sync_once(&lc, &linux).await,
        Err(SyncError::Revoked)
    ));
    // the other device is not affected
    sync_once(&wc, &windows).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_recording_roundtrip() {
    // with the real Winton .ibt (if present): import → sync → other device
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "../../telemetry_examples/iRacing/mx5 mx52016_winton national 2026-11-10 17-00-40.ibt",
    );
    if !file.exists() {
        return;
    }
    let Some(server) = server().await else { return };
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (mut a, b) = (store(da.path()), store(db.path()));
    sy_capture::pipeline::run(
        &mut sy_source_iracing_source(&file),
        &mut a,
        sy_capture::pipeline::RunOptions {
            spool: None,
            should_stop: &|| false,
            idle_done: None,
            progress: None,
        },
    )
    .unwrap();
    let (ca, cb) = (connect(&server, "a").await, connect(&server, "b").await);
    sync_once(&ca, &a).await.unwrap();
    let r = sync_once(&cb, &b).await.unwrap();
    assert_eq!(r.pulled_laps, 5);
    assert_eq!(summary(&a), summary(&b));
}

fn sy_source_iracing_source(file: &std::path::Path) -> impl sy_model::capture::RawSource {
    sy_source_iracing::IbtSource::open(file).unwrap()
}
