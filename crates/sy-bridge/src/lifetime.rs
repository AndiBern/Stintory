// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! When the bridge ends by itself: with the game.
//!
//! Proton starts every game through its helper `steam.exe`, which marks
//! itself as a Wine "system" process with `ProcessWineMakeProcessSystem`
//! (Proton 10.0, `steam_helper/steam.c`) and then waits on the event Wine
//! hands back: it fires when every *non-system* process in the prefix has
//! ended, so games whose launcher exits early keep running in Steam. The
//! bridge, started by Proton before the game, is such a process: the helper
//! waited for the bridge, the bridge for nothing, and Steam showed the game
//! as running for ever. Proton's own kill (`PROTON_REMOTE_DEBUG_CMD`) only
//! comes after the helper returned, and killing the `wine` command doesn't
//! end the Windows process anyway (both seen on Bazzite and reproduced in
//! Wine 11 with `examples/fake_steam.rs`, Nov 2026).
//!
//! There is a second process the helper waits for: Wine runs a program
//! given by a Unix path (as in the launch option) through `start.exe
//! /exec`, which waits for it.
//!
//! So the bridge
//! 1. started by Proton (`PROTON_REMOTE_DEBUG_CMD` in its environment),
//!    continues as a detached copy of itself and the first process ends,
//!    and `start.exe` with it (`main.rs`);
//! 2. once the helper runs, marks itself as a system process as well
//!    (`windows::make_process_system`): the helper then ends with the game;
//! 3. watches the process list and ends a few seconds after the helper is
//!    gone.
//!
//! Without a helper (started by hand, plain Wine) it runs until it is
//! stopped.

/// Proton's helper that runs the game.
pub const GAME_HELPER: &str = "steam.exe";

/// The helper must be missing this many checks in a row (one a second):
/// the process list is a snapshot.
const MISSING_CHECKS: u32 = 3;

/// Decides from the process list when the game is over.
#[derive(Debug, Default, Clone)]
pub struct ExitWithGame {
    seen: bool,
    missing: u32,
}

impl ExitWithGame {
    /// Starts watching; nothing seen yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// One look at the process list. True when the bridge should end.
    pub fn check(&mut self, helper_running: bool) -> bool {
        if helper_running {
            self.seen = true;
            self.missing = 0;
            return false;
        }
        if !self.seen {
            return false;
        }
        self.missing += 1;
        self.missing >= MISSING_CHECKS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ends_only_after_the_helper_was_there_and_is_gone() {
        let mut w = ExitWithGame::new();
        // before the game starts (the bridge starts first): keep going
        for _ in 0..10 {
            assert!(!w.check(false));
        }
        assert!(!w.check(true));
        // one missed snapshot isn't the end
        assert!(!w.check(false));
        assert!(!w.check(true));
        assert!(!w.check(false));
        assert!(!w.check(false));
        assert!(w.check(false), "gone three times in a row");
    }
}
