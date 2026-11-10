-- SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
-- SPDX-License-Identifier: AGPL-3.0-or-later

-- Desktop clients (docs/PLAN.md §9.3): long-lived device tokens, created by
-- the browser-based device login. Only token hashes are stored.
CREATE TABLE device_tokens (
    id            uuid PRIMARY KEY,
    user_id       uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash    bytea NOT NULL UNIQUE,
    name          text NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    last_used_at  timestamptz,
    last_ip       text
);
CREATE INDEX device_tokens_by_user ON device_tokens (user_id);

-- Approved device logins waiting for the client to fetch its token
-- (authorization code + PKCE challenge, single use, short-lived).
CREATE TABLE device_codes (
    code_hash       bytea PRIMARY KEY,
    user_id         uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    code_challenge  text NOT NULL,
    device_name     text NOT NULL,
    expires_at      timestamptz NOT NULL
);

-- Sync (docs/PLAN.md §9.5): every change to a user's sessions and laps gets
-- the next number of one sequence; clients pull "everything after N".
-- Writes of one user are serialized (advisory lock), so numbers become
-- visible in order.
CREATE SEQUENCE change_seq;
ALTER TABLE sessions ADD COLUMN seq bigint NOT NULL DEFAULT nextval('change_seq');
ALTER TABLE laps ADD COLUMN seq bigint NOT NULL DEFAULT nextval('change_seq');
CREATE INDEX sessions_by_seq ON sessions (owner_id, seq);
CREATE INDEX laps_by_seq ON laps (owner_id, seq);

-- Deleted sessions, so other devices delete them too.
CREATE TABLE deletions (
    owner_id  uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    kind      text NOT NULL CHECK (kind IN ('session')),
    id        uuid NOT NULL,
    seq       bigint NOT NULL DEFAULT nextval('change_seq'),
    PRIMARY KEY (owner_id, kind, id)
);
CREATE INDEX deletions_by_seq ON deletions (owner_id, seq);
