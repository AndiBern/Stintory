// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Linux: `LMU_Data` as a file in `/dev/shm`, put there by another
//! shared-memory bridge (simshmbridge's `lmushm` + `lmubridge.exe`,
//! DataLink with a map `LMU_Data`).
//!
//! Such a bridge creates `/dev/shm/LMU_Data` and, inside the game's Wine
//! prefix, a mapping named `LMU_Data` backed by that file, before the game
//! starts. The game then writes into the file's pages, and we map the same
//! pages read-only. Only the memory is shared: the game's events and its
//! lock stay inside Wine, so we poll and check every copy afterwards
//! (see `copy_player`). We never write to the file.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::time::Duration;

/// How often to look for new data. The game writes telemetry at about
/// 100 Hz; polling well above that keeps us from missing frames.
pub(super) const POLL: Duration = Duration::from_millis(2);

/// The file stays when the game exits (simshmbridge keeps it until its
/// `lmushm` is stopped), so its contents may be stale.
pub(super) const OUTLIVES_GAME: bool = true;

/// Where other bridges put LMU's shared memory.
pub const SHM_PATH: &str = "/dev/shm/LMU_Data";

/// Where the data is: a file, normally [`SHM_PATH`].
#[derive(Debug, Clone)]
pub(super) struct Target(pub(super) PathBuf);

impl Default for Target {
    fn default() -> Self {
        Self(PathBuf::from(SHM_PATH))
    }
}

/// A read-only, shared mapping of the file.
pub(super) struct View {
    ptr: *const u8,
    pub(super) len: usize,
}

impl View {
    pub(super) fn open(target: &Target) -> Option<Self> {
        let file = File::open(&target.0).ok()?;
        let len = usize::try_from(file.metadata().ok()?.len()).ok()?;
        if len == 0 {
            // created but not sized (yet): nothing to map, too small anyway
            return Some(Self {
                ptr: std::ptr::null(),
                len: 0,
            });
        }
        // SAFETY: a fresh read-only shared mapping of an open file, `len`
        // bytes long; it stays valid after the file is closed. The file
        // must not shrink while mapped (reads past its end would fault);
        // the bridges size it once when they create it.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return None;
        }
        Some(Self {
            ptr: ptr as *const u8,
            len,
        })
    }

    /// Start of the view; `len` bytes are readable.
    pub(super) fn ptr(&self) -> *const u8 {
        self.ptr
    }
}

impl Drop for View {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: mapped in `open` with this length, unmapped once.
            unsafe {
                libc::munmap(self.ptr as *mut libc::c_void, self.len);
            }
        }
    }
}

/// The game's lock isn't shared with Linux.
pub(super) enum Lock {}

impl Lock {
    pub(super) fn open() -> Option<Self> {
        None
    }

    pub(super) fn acquire(&self, _: Duration) -> bool {
        match *self {}
    }

    pub(super) fn release(&self) {
        match *self {}
    }
}

/// The game's events aren't shared with Linux.
pub(super) enum Events {}

impl Events {
    pub(super) fn open() -> Option<Self> {
        None
    }

    pub(super) fn wait(&self, _: Duration) -> bool {
        match *self {}
    }
}
