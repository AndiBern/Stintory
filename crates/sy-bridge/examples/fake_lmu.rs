// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A stand-in for Le Mans Ultimate on Windows/Wine: publishes the shared
//! memory `LMU_Data` the way the game does (layout from
//! `sy_source_lmu::layout`, data from `sy_source_lmu::sim`), with the
//! Hold/Data events and the writer's lock. Lets the bridge's real LMU
//! reader be tried in a Wine prefix without the game:
//!
//! ```sh
//! cargo build --release -p sy-bridge --example fake_lmu --target x86_64-pc-windows-gnu
//! wine target/x86_64-pc-windows-gnu/release/examples/fake_lmu.exe [laps] [time-scale] &
//! wine target/x86_64-pc-windows-gnu/release/sy-bridge.exe     # same WINEPREFIX
//! cargo run -p sy-bridge --example client                     # native
//! ```
//!
//! It drives an out-lap and `laps` laps (default 2) `time-scale` times
//! faster than real time (default 4), goes back to the garage for 3 s and
//! closes the mapping.

#[cfg(not(windows))]
fn main() {
    eprintln!("fake_lmu is a Windows program; build it with --target x86_64-pc-windows-gnu");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    imp::main();
}

#[cfg(windows)]
#[allow(unsafe_code)] // Win32 shared memory, see SAFETY comments
mod imp {
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::time::{Duration, Instant};

    use sy_source_lmu::layout::{
        OBJECT_SIZE, OFF_GAME_VERSION, OFF_SCORING_INFO, OFF_TELEM_INFO, OFF_TELEMETRY,
        OFF_VEH_SCORING, SCORING_INFO_SIZE, VEH_SCORING_SIZE,
    };
    use sy_source_lmu::sim::SimLmu;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
        PAGE_READWRITE, UnmapViewOfFile,
    };
    use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
    use windows::core::{PCWSTR, w};

    struct Mapping {
        handle: HANDLE,
        view: MEMORY_MAPPED_VIEW_ADDRESS,
        len: usize,
    }

    impl Mapping {
        fn create(name: PCWSTR, len: usize) -> Self {
            // SAFETY: Win32 calls with a valid static name; the view has `len` bytes.
            unsafe {
                let handle = CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    None,
                    PAGE_READWRITE,
                    0,
                    u32::try_from(len).expect("size"),
                    name,
                )
                .expect("CreateFileMappingW");
                let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, len);
                assert!(!view.Value.is_null(), "MapViewOfFile");
                Self { handle, view, len }
            }
        }

        fn write(&self, at: usize, bytes: &[u8]) {
            assert!(at + bytes.len() <= self.len);
            // SAFETY: in bounds (checked); readers synchronise with the lock.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    (self.view.Value as *mut u8).add(at),
                    bytes.len(),
                );
            }
        }

        fn atomic(&self, index: usize) -> &AtomicI32 {
            assert!((index + 1) * 4 <= self.len);
            // SAFETY: page-aligned view, in bounds, shared via interlocked ops only.
            unsafe { AtomicI32::from_ptr((self.view.Value as *mut i32).add(index)) }
        }
    }

    impl Drop for Mapping {
        fn drop(&mut self) {
            // SAFETY: created in `create`, released once.
            unsafe {
                let _ = UnmapViewOfFile(self.view);
                let _ = CloseHandle(self.handle);
            }
        }
    }

    fn event(name: PCWSTR) -> HANDLE {
        // SAFETY: valid static name; auto-reset event.
        unsafe { CreateEventW(None, false, false, name).expect("CreateEventW") }
    }

    /// The game's side of `LMU_SharedMemoryLockData` (`waiters`, `busy`).
    struct Lock {
        data: Mapping,
        event: HANDLE,
    }

    impl Lock {
        fn acquire(&self) {
            let busy = self.data.atomic(1);
            while busy
                .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                self.data.atomic(0).fetch_add(1, Ordering::SeqCst);
                // SAFETY: valid event handle.
                unsafe {
                    WaitForSingleObject(self.event, 5);
                }
                self.data.atomic(0).fetch_sub(1, Ordering::SeqCst);
            }
        }

        fn release(&self) {
            self.data.atomic(1).store(0, Ordering::SeqCst);
            if self.data.atomic(0).load(Ordering::SeqCst) > 0 {
                // SAFETY: valid event handle.
                unsafe {
                    let _ = SetEvent(self.event);
                }
            }
        }
    }

    pub fn main() {
        let mut args = std::env::args().skip(1);
        let laps: i16 = args.next().map_or(2, |a| a.parse().expect("laps"));
        let scale: f64 = args.next().map_or(4.0, |a| a.parse().expect("time scale"));

        let data = Mapping::create(w!("LMU_Data"), OBJECT_SIZE);
        let lock = Lock {
            data: Mapping::create(w!("LMU_SharedMemoryLockData"), 8),
            event: event(w!("LMU_SharedMemoryLockEvent")),
        };
        let hold = event(w!("LMU_Data_HoldEvent"));
        let ready = event(w!("LMU_Data_DataEvent"));
        println!("fake LMU: LMU_Data published ({OBJECT_SIZE} bytes), {laps} laps at {scale}x");

        let mut sim = SimLmu::new();
        let dt = 0.02;
        let mut next_scoring = sim.et;
        let mut garage_until: Option<Instant> = None;
        let mut frames = 0u64;
        loop {
            if garage_until.is_none() && sim.total_laps > laps {
                println!("fake LMU: back to the garage");
                sim.in_realtime = false;
                garage_until = Some(Instant::now() + Duration::from_secs(3));
            }
            if garage_until.is_some_and(|t| Instant::now() >= t) {
                break;
            }
            let _ = sim.drive(dt);
            lock.acquire();
            data.write(OFF_GAME_VERSION, &1_200i32.to_le_bytes());
            if sim.et + 1e-9 >= next_scoring {
                next_scoring += 0.2;
                let rec = sim.scoring();
                let p = &rec.payload[12..];
                data.write(OFF_SCORING_INFO, &p[..SCORING_INFO_SIZE]);
                // the player is the first of 20 cars
                data.write(OFF_VEH_SCORING, &p[SCORING_INFO_SIZE..][..VEH_SCORING_SIZE]);
            }
            if sim.in_realtime {
                // numVehicles active, playerVehicleIdx 0, playerHasVehicle
                data.write(OFF_TELEMETRY, &[1, 0, 1]);
                data.write(OFF_TELEM_INFO, &sim.telemetry());
                frames += 1;
            } else {
                data.write(OFF_TELEMETRY, &[1, 0, 0]);
            }
            lock.release();
            // SAFETY: valid event handles.
            unsafe {
                let _ = SetEvent(hold);
                let _ = SetEvent(ready);
            }
            std::thread::sleep(Duration::from_secs_f64(dt / scale));
        }
        println!("fake LMU: closing after {frames} telemetry frames");
        // SAFETY: created above, released once.
        unsafe {
            let _ = CloseHandle(hold);
            let _ = CloseHandle(ready);
        }
    }
}
