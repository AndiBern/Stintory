-- SPDX-FileCopyrightText: 2026 The Stintory Contributors
-- SPDX-License-Identifier: AGPL-3.0-or-later

-- Refresh tokens that were rotated out (OAuth for MCP clients). One coming
-- back means it was copied: the grant is ended (OAuth 2.1 refresh token
-- reuse detection). Kept as long as a refresh token can live.
CREATE TABLE oauth_used_refresh_tokens (
    token_hash  bytea PRIMARY KEY,
    device_id   uuid NOT NULL REFERENCES device_tokens (id) ON DELETE CASCADE,
    used_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX oauth_used_refresh_tokens_by_device ON oauth_used_refresh_tokens (device_id);
