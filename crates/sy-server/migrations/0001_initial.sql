-- SPDX-FileCopyrightText: 2026 The Stintory Contributors
-- SPDX-License-Identifier: AGPL-3.0-or-later

-- Accounts and authentication (docs/ARCHITECTURE.md §9.3)

CREATE TABLE users (
    id                    uuid PRIMARY KEY,
    -- stored lowercase; unique per server
    username              text NOT NULL UNIQUE CHECK (username = lower(username)),
    display_name          text NOT NULL,
    password_hash         text NOT NULL,
    is_admin              boolean NOT NULL DEFAULT false,
    disabled              boolean NOT NULL DEFAULT false,
    -- set when an admin created the account or reset the password
    must_change_password  boolean NOT NULL DEFAULT false,
    -- TOTP secret, encrypted with the server key; NULL = no TOTP
    totp_secret           bytea,
    -- set once the user confirmed a code; pending setups have NULL
    totp_enabled_at       timestamptz,
    -- last accepted TOTP time step (replay protection)
    totp_last_step        bigint,
    created_at            timestamptz NOT NULL DEFAULT now(),
    updated_at            timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE recovery_codes (
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    code_hash  bytea NOT NULL,
    used_at    timestamptz,
    PRIMARY KEY (user_id, code_hash)
);

-- Browser sessions. The cookie holds a random token; only its hash is stored.
CREATE TABLE web_sessions (
    token_hash    bytea PRIMARY KEY,
    user_id       uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    csrf_token    text NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    last_seen_at  timestamptz NOT NULL DEFAULT now(),
    expires_at    timestamptz NOT NULL,
    user_agent    text,
    ip            text
);
CREATE INDEX web_sessions_by_user ON web_sessions (user_id);

-- Password accepted, second factor pending.
CREATE TABLE login_challenges (
    token_hash  bytea PRIMARY KEY,
    user_id     uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    expires_at  timestamptz NOT NULL,
    attempts    integer NOT NULL DEFAULT 0
);

CREATE TABLE audit_log (
    id       bigserial PRIMARY KEY,
    at       timestamptz NOT NULL DEFAULT now(),
    user_id  uuid REFERENCES users (id) ON DELETE SET NULL,
    event    text NOT NULL,
    ip       text,
    details  jsonb NOT NULL DEFAULT '{}'
);
CREATE INDEX audit_log_by_user ON audit_log (user_id, at DESC);

CREATE TABLE server_settings (
    key    text PRIMARY KEY,
    value  jsonb NOT NULL
);

-- Telemetry data. Ids come from the recording client; the same session can
-- exist for several users (e.g. imported copies), so keys include the owner.

CREATE TABLE blobs (
    hash        text PRIMARY KEY,
    size        bigint NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE sessions (
    owner_id      uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    id            uuid NOT NULL,
    game          text NOT NULL,
    game_version  text,
    track_key     text NOT NULL,
    track_name    text NOT NULL,
    car_key       text NOT NULL,
    car_name      text NOT NULL,
    session_type  text NOT NULL,
    started_at    timestamptz NOT NULL,
    ended_at      timestamptz,
    conditions    jsonb NOT NULL,
    setup         jsonb,
    session_info  jsonb,
    origin        jsonb NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (owner_id, id)
);
CREATE INDEX sessions_by_start ON sessions (owner_id, started_at DESC);
CREATE INDEX sessions_by_track_car ON sessions (game, track_key, car_key);

CREATE TABLE laps (
    owner_id         uuid NOT NULL,
    id               uuid NOT NULL,
    session_id       uuid NOT NULL,
    number           integer NOT NULL,
    lap_time_ms      integer,
    sector_times_ms  jsonb NOT NULL DEFAULT '[]',
    valid            boolean NOT NULL,
    invalid_reason   text,
    flags            jsonb NOT NULL DEFAULT '{}',
    blob_hash        text NOT NULL REFERENCES blobs (hash),
    sample_rate_hz   real NOT NULL,
    sample_count     integer NOT NULL,
    start_time_s     double precision,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (owner_id, id),
    FOREIGN KEY (owner_id, session_id) REFERENCES sessions (owner_id, id) ON DELETE CASCADE
);
CREATE INDEX laps_by_session ON laps (owner_id, session_id, number);
CREATE INDEX laps_by_blob ON laps (blob_hash);
