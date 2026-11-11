// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Imports iRacing `.ibt` and `.stintory` files into a Stintory data
//! directory, without the desktop app (development, bug reports).
//!
//! ```sh
//! cargo run --release -p sy-capture --example import -- <data-dir> <file.ibt|file.stintory>...
//! ```

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use sy_capture::pipeline::{RunOptions, SharedStore, run};
use sy_source_iracing::IbtSource;
use sy_store::Store;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let (Some(data_dir), files) = (args.next(), args.map(PathBuf::from).collect::<Vec<_>>()) else {
        eprintln!("usage: import <data-dir> <file.ibt|file.stintory>...");
        std::process::exit(2);
    };
    if files.is_empty() {
        eprintln!("no files given");
        std::process::exit(2);
    }
    let mut store = SharedStore(Arc::new(Mutex::new(Store::open(PathBuf::from(data_dir))?)));
    let (bundles, ibts): (Vec<PathBuf>, Vec<PathBuf>) = files
        .into_iter()
        .partition(|p| p.extension().is_some_and(|e| e == "stintory"));
    for bundle in &bundles {
        let stats = store.with(|s| s.import_bundle(bundle))?;
        println!("{}: {stats:?}", bundle.display());
    }
    if !ibts.is_empty() {
        let mut source = IbtSource::open_many(&ibts)?;
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
            "imported {} .ibt file(s): {} session(s), {} laps",
            ibts.len(),
            stats.sessions,
            stats.laps
        );
    }
    for s in store.with(|s| s.list_sessions())? {
        println!(
            "  {}  {:<45} {:<25} {:>3} laps",
            s.id, s.track_name, s.car_name, s.lap_count
        );
    }
    Ok(())
}
