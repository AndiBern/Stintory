// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A stand-in for Proton's `steam.exe` (Proton 10.0, `steam_helper/steam.c`)
//! in plain Wine: it starts the game, marks itself as a Wine system process
//! with the game's handle (`NtSetInformationProcess`, class 1000,
//! `ProcessWineMakeProcessSystem`) and waits on the handle it gets back.
//! Wine hands back an event that fires only when every non-system process
//! has ended, so it ends only after the game *and* everything else started
//! in the prefix, e.g. the bridge.
//!
//! Plain Wine starts a program from the command line through `start.exe
//! /exec`, which waits for it and would keep the event from firing; Proton
//! starts its `steam.exe` without one. So this stand-in first continues as
//! a detached copy of itself (the shell gets control back at once; watch
//! for the `steam.exe` process). Copy it to a file named `steam.exe`:
//!
//! ```sh
//! cargo build --release -p sy-bridge --target x86_64-pc-windows-gnu --bin sy-bridge --example fake_steam --example fake_lmu
//! R=$PWD/target/x86_64-pc-windows-gnu/release
//! cp $R/examples/fake_steam.exe /tmp/x/steam.exe
//! # the bridge as Proton starts it (Unix path, PROTON_REMOTE_DEBUG_CMD set)
//! PROTON_REMOTE_DEBUG_CMD=$R/sy-bridge.exe wine $R/sy-bridge.exe &
//! wine 'Z:\tmp\x\steam.exe' "$(winepath -w $R/examples/fake_lmu.exe)" 1 8
//! # steam.exe should end ~5 s after the fake game, the bridge ~3 s later
//! ```
//!
//! Wine processes show their Windows path as their command line, e.g.
//! `ps -eo args | grep '^Z:.*steam.exe'`.

#[cfg(not(windows))]
fn main() {
    eprintln!("fake_steam is a Windows program; build it with --target x86_64-pc-windows-gnu");
    std::process::exit(2);
}

#[cfg(windows)]
#[allow(unsafe_code)] // Win32/NT calls, see SAFETY comments
fn main() {
    use std::os::windows::io::AsRawHandle;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Threading::{GetCurrentProcess, INFINITE, WaitForSingleObject};

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtSetInformationProcess(
            process: HANDLE,
            class: i32,
            info: *mut core::ffi::c_void,
            len: u32,
        ) -> i32;
    }

    if std::env::var_os("FAKE_STEAM_DETACHED").is_none() {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        // the copy runs on alone (no zombies on Windows)
        #[allow(clippy::zombie_processes)]
        std::process::Command::new(std::env::current_exe().expect("own path"))
            .args(std::env::args_os().skip(1))
            .env("FAKE_STEAM_DETACHED", "1")
            .creation_flags(DETACHED_PROCESS)
            .spawn()
            .expect("detach");
        return;
    }
    let mut args = std::env::args().skip(1);
    let game = args.next().expect("usage: steam.exe <game.exe> [args]");
    let child = std::process::Command::new(&game)
        .args(args)
        .spawn()
        .expect("start the game");
    let game_handle = HANDLE(child.as_raw_handle());
    let mut wait = game_handle;
    // SAFETY: as Proton's steam.c does: the buffer is one HANDLE.
    let status = unsafe {
        NtSetInformationProcess(
            GetCurrentProcess(),
            1000,
            (&raw mut wait).cast(),
            size_of::<HANDLE>() as u32,
        )
    };
    println!(
        "fake steam.exe: started {game}; make-system status {status:#x}, waiting on {}",
        if wait == game_handle {
            "the game"
        } else {
            "Wine's event (all non-system processes ended)"
        }
    );
    // SAFETY: a valid handle (the game's, or the event Wine returned).
    unsafe {
        WaitForSingleObject(wait, INFINITE);
    }
    println!("fake steam.exe: done");
}
