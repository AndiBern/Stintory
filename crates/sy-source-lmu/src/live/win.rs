// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Windows (and Wine): the named mapping `LMU_Data`, the Hold/Data events
//! and the writer's lock, as `SharedMemoryInterface.hpp` describes them.

use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, FILE_MAP_READ, MEMORY_BASIC_INFORMATION,
    MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
    VirtualQuery,
};
use windows::Win32::System::Threading::{
    CreateEventW, OpenEventW, SYNCHRONIZATION_SYNCHRONIZE, SetEvent, WaitForSingleObject,
};
use windows::core::w;

/// How often to look for new data when the game has no events (builds
/// before the current interface).
pub(super) const POLL: Duration = Duration::from_millis(10);

/// The mapping goes away with the game (once we close it too), so a mapping
/// that is there belongs to a running game.
pub(super) const OUTLIVES_GAME: bool = false;

/// Where the data is: always the named mapping `LMU_Data`.
#[derive(Debug, Clone, Default)]
pub(super) struct Target {
    _named_mapping: (),
}

/// A read-only view of `LMU_Data`.
pub(super) struct View {
    mapping: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    pub(super) len: usize,
}

impl View {
    pub(super) fn open(_: &Target) -> Option<Self> {
        // SAFETY: plain Win32 calls with valid, static, NUL-terminated names.
        unsafe {
            let mapping = OpenFileMappingW(FILE_MAP_READ.0, false, w!("LMU_Data")).ok()?;
            let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
            if view.Value.is_null() {
                let _ = CloseHandle(mapping);
                return None;
            }
            let mut info = MEMORY_BASIC_INFORMATION::default();
            let written = VirtualQuery(
                Some(view.Value),
                &mut info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            );
            if written == 0 {
                let _ = UnmapViewOfFile(view);
                let _ = CloseHandle(mapping);
                return None;
            }
            Some(Self {
                mapping,
                view,
                len: info.RegionSize,
            })
        }
    }

    /// Start of the view; `len` bytes are readable.
    pub(super) fn ptr(&self) -> *const u8 {
        self.view.Value as *const u8
    }
}

impl Drop for View {
    fn drop(&mut self) {
        // SAFETY: created in `open`, released once.
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.mapping);
        }
    }
}

/// The game's lock around writing the shared memory.
pub(super) struct Lock {
    mapping: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    event: HANDLE,
}

impl Lock {
    pub(super) fn open() -> Option<Self> {
        // SAFETY: Win32 calls with valid static names; the mapping is the
        // size of two LONGs, as in SharedMemoryLock::Init.
        unsafe {
            let mapping = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                8,
                w!("LMU_SharedMemoryLockData"),
            )
            .ok()?;
            let view = MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, 8);
            if view.Value.is_null() {
                let _ = CloseHandle(mapping);
                return None;
            }
            let Ok(event) = CreateEventW(None, false, false, w!("LMU_SharedMemoryLockEvent"))
            else {
                let _ = UnmapViewOfFile(view);
                let _ = CloseHandle(mapping);
                return None;
            };
            Some(Self {
                mapping,
                view,
                event,
            })
        }
    }

    fn waiters(&self) -> &AtomicI32 {
        // SAFETY: the view is 8 bytes, page aligned, and lives as long as
        // self; other processes access it with interlocked operations only.
        unsafe { AtomicI32::from_ptr(self.view.Value as *mut i32) }
    }

    fn busy(&self) -> &AtomicI32 {
        // SAFETY: as in `waiters`; `busy` is the second LONG.
        unsafe { AtomicI32::from_ptr((self.view.Value as *mut i32).add(1)) }
    }

    fn try_acquire(&self) -> bool {
        self.busy()
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Takes the lock, waiting at most `timeout`.
    pub(super) fn acquire(&self, timeout: Duration) -> bool {
        for _ in 0..4000 {
            if self.try_acquire() {
                return true;
            }
            std::hint::spin_loop();
        }
        let start = Instant::now();
        self.waiters().fetch_add(1, Ordering::SeqCst);
        let acquired = loop {
            if self.try_acquire() {
                break true;
            }
            let left = timeout.saturating_sub(start.elapsed());
            if left.is_zero() {
                break false;
            }
            let ms = u32::try_from(left.as_millis().max(1)).unwrap_or(u32::MAX);
            // SAFETY: valid event handle owned by self.
            unsafe {
                WaitForSingleObject(self.event, ms);
            }
        };
        self.waiters().fetch_sub(1, Ordering::SeqCst);
        acquired
    }

    pub(super) fn release(&self) {
        self.busy().store(0, Ordering::SeqCst);
        if self.waiters().load(Ordering::SeqCst) > 0 {
            // SAFETY: valid event handle owned by self.
            unsafe {
                let _ = SetEvent(self.event);
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        // SAFETY: created in `open`, released once.
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.event);
            let _ = CloseHandle(self.mapping);
        }
    }
}

/// `LMU_Data_HoldEvent` and `LMU_Data_DataEvent`.
pub(super) struct Events {
    hold: HANDLE,
    data: HANDLE,
}

impl Events {
    pub(super) fn open() -> Option<Self> {
        // SAFETY: Win32 calls with valid static names.
        unsafe {
            let hold =
                OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, w!("LMU_Data_HoldEvent")).ok()?;
            match OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, w!("LMU_Data_DataEvent")) {
                Ok(data) => Some(Self { hold, data }),
                Err(_) => {
                    let _ = CloseHandle(hold);
                    None
                }
            }
        }
    }

    /// Waits for the next frame (hold, then data), at most `timeout` each.
    pub(super) fn wait(&self, timeout: Duration) -> bool {
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: valid event handles owned by self.
        unsafe {
            WaitForSingleObject(self.hold, ms) == WAIT_OBJECT_0
                && WaitForSingleObject(self.data, ms) == WAIT_OBJECT_0
        }
    }
}

impl Drop for Events {
    fn drop(&mut self) {
        // SAFETY: created in `open`, released once.
        unsafe {
            let _ = CloseHandle(self.hold);
            let _ = CloseHandle(self.data);
        }
    }
}
