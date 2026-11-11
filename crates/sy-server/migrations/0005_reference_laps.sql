-- SPDX-FileCopyrightText: 2026 The Stintory Contributors
-- SPDX-License-Identifier: AGPL-3.0-or-later

-- Reference laps by track and car across all users (docs/PLAN.md §7.5).
-- Track and car keys start with the game, so the game isn't needed.
CREATE INDEX sessions_by_track_key ON sessions (track_key, car_key);
