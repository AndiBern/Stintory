// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The real games, read with the same shared-memory readers as the Windows
//! app (`sy_source_lmu::LiveSource`, `sy_source_iracing::LiveSource`).
//! Inside a Wine/Proton prefix these see the game's named mappings because
//! they run in the game's wineserver.
//!
//! The idle rules match the Windows app's recorders (apps/desktop,
//! `recording.rs`), so a recording ends the same way on both platforms.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sy_model::GameId;
use sy_model::capture::{RawRecord, RawSource, SourceError};

use crate::protocol::GameState;
use crate::server::{Feed, Game};

/// No data for this long while not driving: the session is over (game
/// closed, back in the menus).
const IDLE_END: Duration = Duration::from_secs(10);
/// LMU sends nothing while paused; while driving, wait this long.
const LMU_PAUSE_MAX: Duration = Duration::from_secs(3600);

/// Le Mans Ultimate (`LMU_Data`).
#[derive(Debug, Default)]
pub struct Lmu;

impl Game for Lmu {
    fn id(&self) -> GameId {
        GameId::Lmu
    }

    fn probe(&mut self) -> GameState {
        use sy_source_lmu::Probe;
        match sy_source_lmu::LiveSource::probe() {
            Probe::NotRunning => GameState::NotRunning,
            Probe::NoSharedMemory => GameState::NoSharedMemory,
            Probe::UnknownLayout => GameState::UnknownLayout,
            Probe::Idle => GameState::Idle,
            Probe::Driving => GameState::Driving,
        }
    }

    fn open(&mut self) -> Box<dyn Feed> {
        let source = sy_source_lmu::LiveSource::new();
        let driving = source.driving();
        Box::new(LmuFeed { source, driving })
    }
}

struct LmuFeed {
    source: sy_source_lmu::LiveSource,
    driving: Arc<AtomicBool>,
}

impl Feed for LmuFeed {
    fn next_record(&mut self, timeout: Duration) -> Result<Option<RawRecord>, SourceError> {
        self.source.next_record(timeout)
    }

    fn driving(&self) -> bool {
        self.driving.load(Ordering::Relaxed)
    }

    fn done(&self, idle: Duration) -> bool {
        idle >= if self.driving() {
            LMU_PAUSE_MAX
        } else {
            IDLE_END
        }
    }
}

/// iRacing (`Local\IRSDKMemMapFileName`).
#[derive(Debug, Default)]
pub struct Iracing;

impl Game for Iracing {
    fn id(&self) -> GameId {
        GameId::Iracing
    }

    fn probe(&mut self) -> GameState {
        if sy_source_iracing::LiveSource::probe() {
            GameState::Driving
        } else {
            GameState::NotRunning
        }
    }

    fn open(&mut self) -> Box<dyn Feed> {
        Box::new(IracingFeed(sy_source_iracing::LiveSource::new()))
    }
}

struct IracingFeed(sy_source_iracing::LiveSource);

impl Feed for IracingFeed {
    fn next_record(&mut self, timeout: Duration) -> Result<Option<RawRecord>, SourceError> {
        self.0.next_record(timeout)
    }

    fn driving(&self) -> bool {
        self.0.is_connected()
    }

    fn done(&self, idle: Duration) -> bool {
        idle >= IDLE_END
    }
}

/// Whether a process with this executable name (e.g. `steam.exe`) runs in
/// this Wine prefix / Windows session. Case-insensitive.
#[allow(unsafe_code)] // Win32 process snapshot, see SAFETY comments
pub fn process_running(name: &str) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    // SAFETY: a snapshot handle we own and close; the entry is a plain
    // struct with its size set, as the API requires.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return false;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = false;
        let mut ok = Process32FirstW(snapshot, &mut entry).is_ok();
        while ok {
            let len = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..len]).eq_ignore_ascii_case(name) {
                found = true;
                break;
            }
            ok = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        found
    }
}

/// Wine only: marks this process as a Wine "system" process, like Proton's
/// `steam.exe` marks itself. Proton's `steam.exe` waits until every
/// non-system process in the prefix has ended (the event Wine hands back
/// for `ProcessWineMakeProcessSystem`), so as a normal process the bridge
/// would keep it, and with it the game in Steam, running for ever
/// (`crate::lifetime`). Returns the NTSTATUS on failure (e.g. on Windows,
/// which doesn't know the class).
#[allow(unsafe_code)] // NT call, see SAFETY comment
pub fn make_process_system() -> Result<(), i32> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Threading::GetCurrentProcess;

    /// `ProcessWineMakeProcessSystem` (Wine-specific; Proton's steam.c uses
    /// the same number)
    const PROCESS_WINE_MAKE_PROCESS_SYSTEM: i32 = 1000;

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtSetInformationProcess(
            process: HANDLE,
            class: i32,
            info: *mut core::ffi::c_void,
            len: u32,
        ) -> i32;
    }

    // Wine writes an event handle into the buffer (signalled when no
    // non-system process is left); the bridge doesn't need it.
    let mut event = HANDLE::default();
    // SAFETY: the buffer is one HANDLE, as Wine expects for this class.
    let status = unsafe {
        NtSetInformationProcess(
            GetCurrentProcess(),
            PROCESS_WINE_MAKE_PROCESS_SYSTEM,
            (&raw mut event).cast(),
            size_of::<HANDLE>() as u32,
        )
    };
    if status >= 0 { Ok(()) } else { Err(status) }
}
