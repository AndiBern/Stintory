// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Counting failed login attempts per account and per client address.
//!
//! In memory: a restart resets the counters, which is acceptable for a
//! self-hosted server (and keeps it free of extra infrastructure).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Failed attempts allowed per account within [`WINDOW`].
pub const PER_ACCOUNT: u32 = 10;
/// Failed attempts allowed per client address within [`WINDOW`].
pub const PER_ADDRESS: u32 = 50;
/// Length of the counting window.
pub const WINDOW: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Copy)]
struct Entry {
    failures: u32,
    since: Instant,
}

/// Counters of failed attempts.
#[derive(Debug, Default)]
pub struct RateLimiter {
    entries: Mutex<HashMap<String, Entry>>,
}

impl RateLimiter {
    fn with<T>(&self, f: impl FnOnce(&mut HashMap<String, Entry>) -> T) -> T {
        let mut map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() > 10_000 {
            let now = Instant::now();
            map.retain(|_, e| now.duration_since(e.since) < WINDOW);
        }
        f(&mut map)
    }

    /// True if `key` may try again (fewer than `limit` failures in the window).
    pub fn allowed(&self, key: &str, limit: u32) -> bool {
        self.with(|m| match m.get(key) {
            Some(e) if e.since.elapsed() < WINDOW => e.failures < limit,
            _ => true,
        })
    }

    /// Records a failure for `key`.
    pub fn fail(&self, key: &str) {
        self.with(|m| {
            let now = Instant::now();
            let e = m.entry(key.to_owned()).or_insert(Entry {
                failures: 0,
                since: now,
            });
            if now.duration_since(e.since) >= WINDOW {
                *e = Entry {
                    failures: 0,
                    since: now,
                };
            }
            e.failures += 1;
        });
    }

    /// Forgets `key` (after a successful login).
    pub fn reset(&self, key: &str) {
        self.with(|m| {
            m.remove(key);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_after_limit_and_resets() {
        let r = RateLimiter::default();
        for _ in 0..3 {
            assert!(r.allowed("user:a", 3));
            r.fail("user:a");
        }
        assert!(!r.allowed("user:a", 3));
        assert!(r.allowed("user:b", 3));
        r.reset("user:a");
        assert!(r.allowed("user:a", 3));
    }
}
