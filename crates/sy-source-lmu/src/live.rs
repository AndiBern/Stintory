// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Live data from LMU's shared memory `LMU_Data`.
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
//!
//! Where the memory comes from depends on the platform:
//!
//! - Windows (and `sy-bridge.exe` in Wine): the named mapping, with the
//!   events and the lock (`live/win.rs`);
//! - Linux: the file `/dev/shm/LMU_Data` that another shared-memory bridge
//!   backs the game's mapping with (`live/posix.rs`). There are no events
//!   and no lock, so we poll and check each copy; and since the file stays
//!   when the game exits, we ask the REST API whether the game still runs.

#![allow(unsafe_code)] // reading shared memory; see SAFETY comments

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering, fence};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sy_model::GameId;
use sy_model::capture::{RawRecord, RawSource, SourceError};

use crate::layout::{
    MAX_VEHICLES, OBJECT_SIZE, OFF_GAME_VERSION, OFF_SCORING_INFO, OFF_TELEM_INFO, OFF_TELEMETRY,
    OFF_VEH_SCORING, SCORING_INFO_SIZE, TELEM_SIZE, VEH_SCORING_SIZE, off,
};
use crate::records::{
    elapsed_time, in_realtime, num_vehicles, scoring_record, setup_record, slot_id,
    telemetry_record,
};
use crate::rest::{REST_ADDR, SetupPoller, game_running};

#[cfg(unix)]
#[path = "live/posix.rs"]
mod sys;
#[cfg(windows)]
#[path = "live/win.rs"]
mod sys;

#[cfg(unix)]
pub use sys::SHM_PATH;
use sys::{Events, Lock, Target, View};

/// No new data for this long: on Linux, check whether the game still runs.
const STALE_AFTER: Duration = Duration::from_secs(3);
/// How often to ask the REST API while the data is stale.
const STALE_CHECK_EVERY: Duration = Duration::from_secs(2);

/// What [`LiveSource::probe`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// LMU isn't running.
    NotRunning,
    /// LMU runs, but there is no shared memory: *Enable Plugins* is off (or
    /// the game was not restarted after turning it on). On Linux also: the
    /// other bridge didn't create `/dev/shm/LMU_Data`.
    NoSharedMemory,
    /// The shared memory is smaller than this version of Stintory knows:
    /// the game's layout changed (on Linux also: the other bridge made the
    /// file too small).
    UnknownLayout,
    /// Linux: LMU runs and `/dev/shm/LMU_Data` exists, but the game doesn't
    /// write into it: the other bridge started after the game, or
    /// *Enable Plugins* is off.
    Unlinked,
    /// The shared memory exists, the player isn't driving.
    Idle,
    /// The player is driving.
    Driving,
}

impl View {
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
            std::ptr::copy_nonoverlapping(self.ptr().add(offset), out.as_mut_ptr(), len);
        }
        fence(Ordering::Acquire);
        Some(out)
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
    fn open(target: &Target) -> Option<Self> {
        let view = View::open(target)?;
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
            None => std::thread::sleep(sys::POLL.min(timeout)),
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
        if self.lock.is_none() {
            // no lock: the game may have written while we copied; the copy
            // is only good if a second one is the same
            if let Some((i, t)) = &telemetry
                && v.copy(OFF_TELEM_INFO + i * TELEM_SIZE, TELEM_SIZE)? != *t
            {
                return None;
            }
            if v.copy(OFF_SCORING_INFO, SCORING_INFO_SIZE)? != info {
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

/// Whether LMU runs (its REST API answers).
fn lmu_running() -> bool {
    game_running(REST_ADDR, Duration::from_millis(300))
}

/// Whether the player's telemetry moves on within a few frames.
fn advancing(c: &Connection, first: &Snapshot) -> bool {
    let et = |s: &Snapshot| s.telemetry.as_deref().map(elapsed_time);
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(200) {
        std::thread::sleep(sys::POLL * 5);
        if let Some(s) = c.snapshot(None)
            && et(&s).is_some()
            && et(&s) != et(first)
        {
            return true;
        }
    }
    false
}

fn probe(target: &Target) -> Probe {
    let view = View::open(target);
    if view.as_ref().is_some_and(|v| v.len < OBJECT_SIZE) {
        return Probe::UnknownLayout;
    }
    if sys::OUTLIVES_GAME {
        // the file may be left over from an earlier session
        if !lmu_running() {
            return Probe::NotRunning;
        }
        if view.is_none() {
            return Probe::NoSharedMemory;
        }
    }
    drop(view);
    match Connection::open(target) {
        Some(c) => match c.snapshot(None) {
            // the game writes its version first thing; a file it doesn't
            // write into stays zero
            Some(s) if sys::OUTLIVES_GAME && s.game_version == 0 => Probe::Unlinked,
            Some(s) if in_realtime(&s.info) && s.telemetry.is_some() => {
                if sys::OUTLIVES_GAME && !advancing(&c, &s) {
                    // left over from an earlier session (or paused)
                    Probe::Idle
                } else {
                    Probe::Driving
                }
            }
            _ => Probe::Idle,
        },
        None if lmu_running() => Probe::NoSharedMemory,
        None => Probe::NotRunning,
    }
}

/// [`RawSource`] reading LMU's shared memory, plus the car setup from the
/// REST API.
pub struct LiveSource {
    target: Target,
    conn: Option<Connection>,
    pending: VecDeque<RawRecord>,
    last_scoring: Option<Vec<u8>>,
    last_et: Option<f64>,
    player_slot: Option<i32>,
    driving: Arc<AtomicBool>,
    setup: Option<SetupPoller>,
    /// When the data last changed
    last_change: Instant,
    /// When we last asked the REST API whether the game runs
    last_check: Option<Instant>,
    /// The game is gone (Linux: the data in the file is stale)
    gone: bool,
}

impl std::fmt::Debug for LiveSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveSource")
            .field("target", &self.target)
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
        Self::with_target(Target::default())
    }

    fn with_target(target: Target) -> Self {
        Self {
            target,
            conn: None,
            pending: VecDeque::new(),
            last_scoring: None,
            last_et: None,
            player_slot: None,
            driving: Arc::default(),
            setup: None,
            last_change: Instant::now(),
            last_check: None,
            gone: false,
        }
    }

    /// Whether LMU runs and the player drives.
    pub fn probe() -> Probe {
        probe(&Target::default())
    }

    /// True while the player drives (updated with every read). The recording
    /// waits longer for data then: the game sends nothing while paused.
    pub fn driving(&self) -> Arc<AtomicBool> {
        self.driving.clone()
    }

    /// Linux: no new data for a while and the game doesn't answer anymore.
    fn stale(&mut self) -> bool {
        if !sys::OUTLIVES_GAME || self.last_change.elapsed() < STALE_AFTER {
            self.gone = false;
            return false;
        }
        if !self.gone
            && self
                .last_check
                .is_none_or(|t| t.elapsed() >= STALE_CHECK_EVERY)
        {
            self.last_check = Some(Instant::now());
            self.gone = !lmu_running();
            if self.gone {
                tracing::info!("Le Mans Ultimate is gone; its shared memory is stale");
            }
        }
        self.gone
    }

    fn poll(&mut self, timeout: Duration) {
        if self.conn.is_none() {
            self.conn = Connection::open(&self.target);
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
        let mut driving = realtime && snap.telemetry.is_some();

        if let Some(player) = &snap.player_scoring {
            self.player_slot = Some(slot_id(player));
            let record = scoring_record(received, snap.game_version, &snap.info, player);
            // the receive time differs every time; compare the rest
            if self.last_scoring.as_deref() != Some(&record.payload[8..]) {
                self.last_scoring = Some(record.payload[8..].to_vec());
                self.last_change = Instant::now();
                self.pending.push_back(record);
            }
        }
        if let Some(t) = snap.telemetry {
            let et = elapsed_time(&t);
            if self.last_et != Some(et) {
                self.last_et = Some(et);
                self.last_change = Instant::now();
                self.pending.push_back(telemetry_record(&t));
            }
        }
        if self.stale() {
            driving = false;
        }
        self.driving.store(driving, Ordering::Relaxed);
    }
}

#[cfg(unix)]
impl LiveSource {
    /// A source reading another file than [`SHM_PATH`].
    pub fn at(path: impl Into<std::path::PathBuf>) -> Self {
        Self::with_target(Target(path.into()))
    }

    /// [`LiveSource::probe`] for another file than [`SHM_PATH`].
    pub fn probe_at(path: impl Into<std::path::PathBuf>) -> Probe {
        probe(&Target(path.into()))
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
