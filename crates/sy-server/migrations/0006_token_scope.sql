-- SPDX-FileCopyrightText: 2026 The Stintory Contributors
-- SPDX-License-Identifier: AGPL-3.0-or-later

-- What a device token may do: 'sync' = the desktop app (full access),
-- 'read' = an AI assistant through the MCP endpoint (reads only, no other
-- API).
ALTER TABLE device_tokens
    ADD COLUMN scope text NOT NULL DEFAULT 'sync' CHECK (scope IN ('sync', 'read'));
