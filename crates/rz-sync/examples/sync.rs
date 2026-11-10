// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Syncs a Rundenzeit data directory with a server, without the desktop app
//! (development, headless machines). The first run connects through the
//! browser login and keeps the device token in the data directory.
//!
//! ```sh
//! cargo run -p rz-sync --example sync -- <data-dir> <server-url>
//! ```

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rz_capture::pipeline::SharedStore;
use rz_store::{SYNC_SERVER_URL, SYNC_TOKEN_FALLBACK, Store};
use rz_sync::{Client, DeviceLogin, sync_once};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(dir), Some(server)) = (args.next(), args.next()) else {
        eprintln!("usage: sync <data-dir> <server-url>");
        std::process::exit(2);
    };
    let store = SharedStore(Arc::new(Mutex::new(Store::open(PathBuf::from(dir))?)));
    let client = Client::new(&server)?;
    let saved: Option<String> = store.with(|s| {
        if s.setting::<String>(SYNC_SERVER_URL)?.as_deref() == Some(client.server_url()) {
            s.setting(SYNC_TOKEN_FALLBACK)
        } else {
            Ok(None)
        }
    })?;
    let token = match saved {
        Some(t) => t,
        None => {
            let (login, url) = DeviceLogin::start(client.clone(), "rz-sync example").await?;
            println!("Open this address in a browser to connect:\n{url}");
            let device = login.finish(Duration::from_secs(600)).await?;
            println!("connected as @{}", device.user.username);
            store.with(|s| {
                s.reset_sync()?;
                s.set_setting(SYNC_SERVER_URL, &client.server_url())?;
                s.set_setting(SYNC_TOKEN_FALLBACK, &device.token)
            })?;
            device.token
        }
    };
    let report = sync_once(&client.with_token(token), &store).await?;
    println!("{report:?}");
    Ok(())
}
