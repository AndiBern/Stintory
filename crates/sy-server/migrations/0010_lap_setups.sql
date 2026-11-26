-- SPDX-FileCopyrightText: 2026 The Stintory Contributors
-- SPDX-License-Identifier: AGPL-3.0-or-later

-- The car setup each lap was driven with (game-specific JSON, like
-- sessions.setup). Laps recorded before have none.
ALTER TABLE laps ADD COLUMN setup jsonb;

-- A user or team grant can include the setups (docs/ARCHITECTURE.md §9.2).
-- Never for everyone on the server.
ALTER TABLE share_grants ADD COLUMN with_setup boolean NOT NULL DEFAULT false;
ALTER TABLE share_grants ADD CONSTRAINT share_grants_setup_not_everyone
    CHECK (NOT with_setup OR grantee_kind <> 'everyone');
