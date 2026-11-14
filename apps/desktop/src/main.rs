// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stintory desktop client entry point.

// Don't open a console window next to the app in Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `stintory mcp`: MCP server for AI assistants on stdio, no window
    if std::env::args_os().nth(1).is_some_and(|a| a == "mcp") {
        std::process::exit(stintory_desktop::run_mcp());
    }
    stintory_desktop::run();
}
