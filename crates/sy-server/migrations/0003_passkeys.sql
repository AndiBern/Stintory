-- SPDX-FileCopyrightText: 2026 The Stintory Contributors
-- SPDX-License-Identifier: AGPL-3.0-or-later

-- Passkeys (WebAuthn, docs/PLAN.md §9.3): passwordless login or second factor.
CREATE TABLE passkeys (
    id             uuid PRIMARY KEY,
    user_id        uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- a credential belongs to exactly one account
    credential_id  bytea NOT NULL UNIQUE,
    name           text NOT NULL,
    -- the public key and signature counter (webauthn-rs `Passkey`)
    passkey        jsonb NOT NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    last_used_at   timestamptz
);
CREATE INDEX passkeys_by_user ON passkeys (user_id);

-- Running WebAuthn ceremonies. The state must stay on the server (replay
-- protection); the client only gets the id. Single use, short-lived.
CREATE TABLE webauthn_challenges (
    token_hash  bytea PRIMARY KEY,
    user_id     uuid REFERENCES users (id) ON DELETE CASCADE,
    purpose     text NOT NULL CHECK (purpose IN ('register', 'login', 'second_factor')),
    state       jsonb NOT NULL,
    -- purpose-specific: passkey name, login challenge
    extra       jsonb NOT NULL DEFAULT '{}',
    expires_at  timestamptz NOT NULL
);
