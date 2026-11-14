// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Live data from LMU's shared memory `LMU_Data` (Windows only).
//!
//! Source: `SharedMemoryInterface.hpp` from the LMU install (see
//! [`crate::layout`]). The protocol it describes:
//!
//! 1. open the file mapping `LMU_Data` (read-only is enough);
//! 2. wait for a new frame: first on the event `LMU_Data_HoldEvent`, then on
//!    `LMU_Data_DataEvent` (that order; builds before this interface have no
//!    events, then we poll);
//! 3. take the lock `LMU_SharedMemoryLockData` (two `LONG`s, `waiters` and
//!    `busy`, in a small mapping, plus the auto-reset event
//!    `LMU_SharedMemoryLockEvent`): spin on `busy` 0 → 1, then wait on the
//!    event as a registered waiter; release with `busy` = 0 and set the
//!    event if someone waits;
//! 4. copy what is needed, release the lock.
//!
//! Under the lock we copy only the session scoring, the player's scoring and
//! the player's telemetry, never other cars' data (docs/ARCHITECTURE.md §5.2). Our
//! lock waits are bounded (the game must never wait on us for long).
//!
//! The player's telemetry is `telemInfo[playerVehicleIdx]`; its slot id must
//! match the player's scoring entry (`mIsPlayer`), otherwise the slots are
//! searched by id.

#![allow(unsafe_code)] // FFI and reading shared memory; see SAFETY comments

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering, fence};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sy_model::GameId;
use sy_model::capture::{RawRecord, RawSource, SourceError};
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

use crate::layout::{
    MAX_VEHICLES, OBJECT_SIZE, OFF_GAME_VERSION, OFF_SCORING_INFO, OFF_TELEM_INFO, OFF_TELEMETRY,
    OFF_VEH_SCORING, SCORING_INFO_SIZE, TELEM_SIZE, VEH_SCORING_SIZE, off,
};
use crate::records::{
    elapsed_time, in_realtime, num_vehicles, scoring_record, setup_record, slot_id,
    telemetry_record,
};
use crate::rest::{REST_ADDR, SetupPoller, game_running};

/// What [`LiveSource::probe`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// LMU isn't running.
    NotRunning,
    /// LMU runs, but there is no shared memory: *Enable Plugins* is off (or
    /// the game was not restarted after turning it on).
    NoSharedMemory,
    /// The shared memory is smaller than this version of Stintory knows:
    /// the game's layout changed.
    UnknownLayout,
    /// The shared memory exists, the player isn't driving.
    Idle,
    /// The player is driving.
    Driving,
}

/// A read-only view of a named mapping.
struct View {
    mapping: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    len: usize,
}

impl View {
    fn open_lmu_data() -> Option<Self> {
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

    /// Copies `len` bytes at `offset`.
    fn copy(&self, offset: usize, len: usize) -> Option<Vec<u8>> {
        let end = offset.checked_add(len)?;
        if end > self.len {
            return None;
        }
        let mut out = vec![0u8; len];
        fence(Ordering::Acquire);
        // SAFETY: the range was checked against the size of the mapped
        // region. The game may write concurrently unless we hold its lock;
        // without the lock callers check the data afterwards.
        unsafe {
            std::ptr::copy_nonoverlapping(
                (self.view.Value as *const u8).add(offset),
                out.as_mut_ptr(),
                len,
            );
        }
        fence(Ordering::Acquire);
        Some(out)
    }
}

impl Drop for View {
    fn drop(&mut self) {
        // SAFETY: created in `open_lmu_data`, released once.
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.mapping);
        }
    }
}

/// The game's lock around writing the shared memory.
struct Lock {
    mapping: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    event: HANDLE,
}

impl Lock {
    fn open() -> Option<Self> {
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
    fn acquire(&self, timeout: Duration) -> bool {
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

    fn release(&self) {
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
struct Events {
    hold: HANDLE,
    data: HANDLE,
}

impl Events {
    fn open() -> Option<Self> {
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
    fn wait(&self, timeout: Duration) -> bool {
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

struct Connection {
    view: View,
    events: Option<Events>,
    lock: Option<Lock>,
}

// SAFETY: handles and mapped views are process-wide and may be used from any
// thread; the lock's atomics are shared memory by design.
unsafe impl Send for Connection {}

/// One copy of what we read, taken under the lock.
struct Snapshot {
    game_version: i32,
    info: Vec<u8>,
    player_scoring: Option<Vec<u8>>,
    telemetry: Option<Vec<u8>>,
}

impl Connection {
    fn open() -> Option<Self> {
        let view = View::open_lmu_data()?;
        if view.len < OBJECT_SIZE {
            tracing::warn!(
                size = view.len,
                expected = OBJECT_SIZE,
                "LMU_Data is smaller than expected; the game's layout changed"
            );
            return None;
        }
        Some(Self {
            view,
            events: Events::open(),
            lock: Lock::open(),
        })
    }

    /// Waits for new data (or polls when the game has no events).
    fn wait(&self, timeout: Duration) {
        match &self.events {
            Some(e) => {
                e.wait(timeout);
            }
            None => std::thread::sleep(Duration::from_millis(10).min(timeout)),
        }
    }

    /// Copies the player's data.
    fn snapshot(&self, player_slot: Option<i32>) -> Option<Snapshot> {
        let locked = match &self.lock {
            Some(l) => {
                if !l.acquire(Duration::from_millis(20)) {
                    return None;
                }
                true
            }
            None => false,
        };
        let snap = self.copy_player(player_slot);
        if locked {
            self.lock.as_ref().expect("locked").release();
        }
        snap
    }

    fn copy_player(&self, player_slot: Option<i32>) -> Option<Snapshot> {
        let v = &self.view;
        let version = v.copy(OFF_GAME_VERSION, 4)?;
        let info = v.copy(OFF_SCORING_INFO, SCORING_INFO_SIZE)?;
        let n = usize::try_from(num_vehicles(&info))
            .unwrap_or(0)
            .min(MAX_VEHICLES);
        let mut player_scoring = None;
        for i in 0..n {
            let at = OFF_VEH_SCORING + i * VEH_SCORING_SIZE;
            // read the flag only, then copy the player's entry
            if v.copy(at + off::veh::IS_PLAYER, 1)
                .is_some_and(|b| b[0] != 0)
            {
                player_scoring = v.copy(at, VEH_SCORING_SIZE);
                break;
            }
        }
        let head = v.copy(OFF_TELEMETRY, 3)?;
        let (active, idx, has_vehicle) = (usize::from(head[0]), usize::from(head[1]), head[2] != 0);
        let slot = player_scoring.as_deref().map(slot_id).or(player_slot);
        let mut telemetry = None;
        if has_vehicle {
            let at = |i: usize| OFF_TELEM_INFO + i * TELEM_SIZE;
            let mut index = (idx < MAX_VEHICLES).then_some(idx);
            if let (Some(i), Some(slot)) = (index, slot)
                && v.copy(at(i), 4).map(|b| slot_id(&b)) != Some(slot)
            {
                index = (0..active.min(MAX_VEHICLES))
                    .find(|&j| v.copy(at(j), 4).map(|b| slot_id(&b)) == Some(slot));
            }
            telemetry = index.and_then(|i| v.copy(at(i), TELEM_SIZE).map(|t| (i, t)));
        }
        if self.lock.is_none()
            && let Some((i, t)) = &telemetry
        {
            // no lock: the copy is only good if the time didn't move
            let again = v.copy(OFF_TELEM_INFO + i * TELEM_SIZE, TELEM_SIZE)?;
            if elapsed_time(&again) != elapsed_time(t) {
                return None;
            }
        }
        Some(Snapshot {
            game_version: i32::from_le_bytes(version.try_into().ok()?),
            info,
            player_scoring,
            telemetry: telemetry.map(|(_, t)| t),
        })
    }
}

/// [`RawSource`] reading LMU's shared memory, plus the car setup from the
/// REST API.
pub struct LiveSource {
    conn: Option<Connection>,
    pending: VecDeque<RawRecord>,
    last_scoring: Option<Vec<u8>>,
    last_et: Option<f64>,
    player_slot: Option<i32>,
    driving: Arc<AtomicBool>,
    setup: Option<SetupPoller>,
}

impl std::fmt::Debug for LiveSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveSource")
            .field("connected", &self.conn.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for LiveSource {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveSource {
    /// Creates a source; it connects lazily.
    pub fn new() -> Self {
        Self {
            conn: None,
            pending: VecDeque::new(),
            last_scoring: None,
            last_et: None,
            player_slot: None,
            driving: Arc::default(),
            setup: None,
        }
    }

    /// Whether LMU runs and the player drives.
    pub fn probe() -> Probe {
        if View::open_lmu_data().is_some_and(|v| v.len < OBJECT_SIZE) {
            return Probe::UnknownLayout;
        }
        match Connection::open() {
            Some(c) => match c.snapshot(None) {
                Some(s) if in_realtime(&s.info) && s.telemetry.is_some() => Probe::Driving,
                _ => Probe::Idle,
            },
            None if game_running(REST_ADDR, Duration::from_millis(300)) => Probe::NoSharedMemory,
            None => Probe::NotRunning,
        }
    }

    /// True while the player drives (updated with every read). The recording
    /// waits longer for data then: the game sends nothing while paused.
    pub fn driving(&self) -> Arc<AtomicBool> {
        self.driving.clone()
    }

    fn poll(&mut self, timeout: Duration) {
        if self.conn.is_none() {
            self.conn = Connection::open();
            if self.conn.is_none() {
                self.driving.store(false, Ordering::Relaxed);
                std::thread::sleep(timeout.min(Duration::from_secs(1)));
                return;
            }
            self.setup = Some(SetupPoller::start(REST_ADDR, Duration::from_secs(5)));
        }
        if let Some(json) = self.setup.as_ref().and_then(SetupPoller::changed) {
            self.pending.push_back(setup_record(&json));
        }
        let conn = self.conn.as_ref().expect("connected");
        conn.wait(timeout);
        let Some(snap) = conn.snapshot(self.player_slot) else {
            return;
        };
        let received = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let realtime = in_realtime(&snap.info);
        self.driving
            .store(realtime && snap.telemetry.is_some(), Ordering::Relaxed);

        if let Some(player) = &snap.player_scoring {
            self.player_slot = Some(slot_id(player));
            let record = scoring_record(received, snap.game_version, &snap.info, player);
            // the receive time differs every time; compare the rest
            if self.last_scoring.as_deref() != Some(&record.payload[8..]) {
                self.last_scoring = Some(record.payload[8..].to_vec());
                self.pending.push_back(record);
            }
        }
        if let Some(t) = snap.telemetry {
            let et = elapsed_time(&t);
            if self.last_et != Some(et) {
                self.last_et = Some(et);
                self.pending.push_back(telemetry_record(&t));
            }
        }
    }
}

impl RawSource for LiveSource {
    fn game(&self) -> GameId {
        GameId::Lmu
    }

    fn next_record(&mut self, timeout: Duration) -> Result<Option<RawRecord>, SourceError> {
        if self.pending.is_empty() {
            self.poll(timeout);
        }
        Ok(self.pending.pop_front())
    }
}
