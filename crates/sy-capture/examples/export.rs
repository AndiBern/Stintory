// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Exports all sessions of a Stintory data directory to an `.stintory` file,
//! without the desktop app (development, bug reports).
//!
//! ```sh
//! cargo run --release -p sy-capture --example export -- <data-dir> <out.stintory> [driver name]
//! ```

use std::path::PathBuf;

use sy_store::{ExportOptions, ExportSelection, Store};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let (Some(data_dir), Some(out)) = (args.next(), args.next()) else {
        eprintln!("usage: export <data-dir> <out.stintory> [driver name]");
        std::process::exit(2);
    };
    let driver_name = args.next().map(|n| n.to_string_lossy().into_owned());
    let store = Store::open(PathBuf::from(data_dir))?;
    let sessions = store.list_sessions()?.into_iter().map(|s| s.id).collect();
    let stats = store.export_bundle(
        &PathBuf::from(out),
        &ExportSelection {
            sessions,
            laps: Vec::new(),
        },
        &ExportOptions {
            exporter: format!("Stintory {} (export example)", env!("CARGO_PKG_VERSION")),
            driver_name,
            include_setup: false,
        },
    )?;
    println!(
        "exported {} session(s), {} laps",
        stats.sessions, stats.laps
    );
    Ok(())
}
