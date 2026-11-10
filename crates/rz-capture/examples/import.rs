// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Imports iRacing `.ibt` files into a Rundenzeit data directory, without
//! the desktop app (development, bug reports).
//!
//! ```sh
//! cargo run --release -p rz-capture --example import -- <data-dir> <file.ibt>...
//! ```

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rz_capture::pipeline::{RunOptions, SharedStore, run};
use rz_source_iracing::IbtSource;
use rz_store::Store;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let (Some(data_dir), files) = (args.next(), args.map(PathBuf::from).collect::<Vec<_>>()) else {
        eprintln!("usage: import <data-dir> <file.ibt>...");
        std::process::exit(2);
    };
    if files.is_empty() {
        eprintln!("no files given");
        std::process::exit(2);
    }
    let mut store = SharedStore(Arc::new(Mutex::new(Store::open(PathBuf::from(data_dir))?)));
    let mut source = IbtSource::open_many(&files)?;
    let stats = run(
        &mut source,
        &mut store,
        RunOptions {
            spool: None,
            should_stop: &|| false,
            idle_done: None,
            progress: None,
        },
    )?;
    println!(
        "imported {} file(s): {} session(s), {} laps",
        files.len(),
        stats.sessions,
        stats.laps
    );
    for s in store.with(|s| s.list_sessions())? {
        println!(
            "  {}  {:<45} {:<25} {:>3} laps",
            s.id, s.track_name, s.car_name, s.lap_count
        );
    }
    Ok(())
}
