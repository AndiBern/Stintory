// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A simulated Le Mans Ultimate for the bridge ([`SimGame`]): drives
//! [`sy_source_lmu::sim::SimLmu`] in real time (or faster) and then
//! "closes the game". Used by the tests and by `sy-bridge --simulate-lmu`,
//! which lets the whole path (bridge → client → store) be tried on Linux
//! without the game.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sy_model::GameId;
use sy_model::capture::{RawRecord, SourceError};
use sy_source_lmu::sim::SimLmu;

use crate::protocol::GameState;
use crate::server::{Feed, Game};

/// A simulated LMU session: out-lap from the pit lane, then `laps` laps.
#[derive(Debug, Clone)]
pub struct SimGame {
    laps: i16,
    time_scale: f64,
    idle_end: Duration,
    closed: Arc<AtomicBool>,
}

impl SimGame {
    /// `laps` timed laps; `time_scale` game seconds per real second.
    pub fn new(laps: u16, time_scale: f64) -> Self {
        Self {
            laps: i16::try_from(laps).unwrap_or(i16::MAX),
            time_scale: time_scale.max(0.01),
            idle_end: Duration::from_secs(10),
            closed: Arc::default(),
        }
    }

    /// How long silence ends the recording once the session is over (10 s,
    /// as for the real game).
    pub fn with_idle_end(mut self, idle_end: Duration) -> Self {
        self.idle_end = idle_end;
        self
    }
}

impl Game for SimGame {
    fn id(&self) -> GameId {
        GameId::Lmu
    }

    fn probe(&mut self) -> GameState {
        if self.closed.load(Ordering::Relaxed) {
            GameState::NotRunning
        } else {
            GameState::Driving
        }
    }

    fn open(&mut self) -> Box<dyn Feed> {
        let sim = SimLmu::new();
        Box::new(SimFeed {
            start_et: sim.et,
            sim,
            started: Instant::now(),
            pending: VecDeque::from([SimLmu::setup(r#"{"VM_BRAKE_BALANCE":"52.0:48.0"}"#)]),
            game: self.clone(),
        })
    }
}

struct SimFeed {
    sim: SimLmu,
    start_et: f64,
    started: Instant,
    pending: VecDeque<RawRecord>,
    game: SimGame,
}

impl SimFeed {
    fn finished(&self) -> bool {
        self.sim.total_laps > self.game.laps
    }
}

impl Feed for SimFeed {
    fn next_record(&mut self, timeout: Duration) -> Result<Option<RawRecord>, SourceError> {
        if self.pending.is_empty() && !self.finished() {
            let target =
                self.start_et + self.started.elapsed().as_secs_f64() * self.game.time_scale;
            if target > self.sim.et {
                self.pending.extend(self.sim.drive(target - self.sim.et));
            }
            if self.finished() {
                // a little past the line, so the lap's time is in a scoring
                self.pending.extend(self.sim.drive(1.0));
                tracing::info!("simulated session over; the game \"closes\"");
                self.game.closed.store(true, Ordering::Relaxed);
            }
        }
        if self.pending.is_empty() {
            std::thread::sleep(timeout.min(Duration::from_millis(5)));
        }
        Ok(self.pending.pop_front())
    }

    fn driving(&self) -> bool {
        !self.finished()
    }

    fn done(&self, idle: Duration) -> bool {
        self.finished() && idle >= self.game.idle_end
    }
}
