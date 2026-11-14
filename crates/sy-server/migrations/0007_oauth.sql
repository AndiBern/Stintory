-- SPDX-FileCopyrightText: 2026 The Stintory Contributors
-- SPDX-License-Identifier: AGPL-3.0-or-later

-- OAuth 2.1 for MCP clients (AI assistants, docs/SERVER.md).

-- Clients: registered dynamically (RFC 7591, client_id = uuid) or known
-- from their client ID metadata document (client_id = its https URL,
-- cached).
CREATE TABLE oauth_clients (
    client_id      text PRIMARY KEY,
    client_name    text NOT NULL,
    redirect_uris  jsonb NOT NULL,
    source         text NOT NULL CHECK (source IN ('registered', 'metadata_document')),
    created_at     timestamptz NOT NULL DEFAULT now(),
    fetched_at     timestamptz
);

-- An approved authorization is a read-only device token row: listed on
-- the Account page, removed there. Its token_hash is the hash of the
-- current refresh token (rotated on every use); it is never accepted as a
-- bearer token itself.
ALTER TABLE device_tokens
    ADD COLUMN oauth_client text,
    ADD COLUMN refresh_expires_at timestamptz;

-- Authorization codes: single use, short-lived, PKCE (S256).
CREATE TABLE oauth_codes (
    code_hash       bytea PRIMARY KEY,
    user_id         uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    client_id       text NOT NULL,
    redirect_uri    text NOT NULL,
    code_challenge  text NOT NULL,
    resource        text NOT NULL,
    expires_at      timestamptz NOT NULL
);

-- Access tokens: short-lived, for one resource (the MCP endpoint).
CREATE TABLE oauth_access_tokens (
    token_hash  bytea PRIMARY KEY,
    device_id   uuid NOT NULL REFERENCES device_tokens (id) ON DELETE CASCADE,
    resource    text NOT NULL,
    expires_at  timestamptz NOT NULL
);
CREATE INDEX oauth_access_tokens_by_device ON oauth_access_tokens (device_id);
