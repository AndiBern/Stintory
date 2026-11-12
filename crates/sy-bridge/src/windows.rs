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
