-- SPDX-FileCopyrightText: 2026 The Stintory Contributors
-- SPDX-License-Identifier: AGPL-3.0-or-later

-- Teams and sharing (docs/PLAN.md §9.2)

CREATE TABLE teams (
    id          uuid PRIMARY KEY,
    name        text NOT NULL,
    created_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE team_members (
    team_id    uuid NOT NULL REFERENCES teams (id) ON DELETE CASCADE,
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role       text NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
    joined_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (team_id, user_id)
);
CREATE INDEX team_members_by_user ON team_members (user_id);

-- Invitations: to one user (accept/decline), or a link anyone logged in can
-- use (only the token's hash is stored). "Expired" is derived from
-- expires_at.
CREATE TABLE team_invites (
    id          uuid PRIMARY KEY,
    team_id     uuid NOT NULL REFERENCES teams (id) ON DELETE CASCADE,
    invited_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    kind        text NOT NULL CHECK (kind IN ('user', 'link')),
    invitee_id  uuid REFERENCES users (id) ON DELETE CASCADE,
    token_hash  bytea UNIQUE,
    max_uses    integer,
    uses        integer NOT NULL DEFAULT 0,
    expires_at  timestamptz,
    status      text NOT NULL DEFAULT 'pending'
                CHECK (status IN ('pending', 'accepted', 'declined', 'revoked')),
    created_at  timestamptz NOT NULL DEFAULT now(),
    CHECK ((kind = 'user' AND invitee_id IS NOT NULL) OR (kind = 'link' AND token_hash IS NOT NULL))
);
CREATE INDEX team_invites_by_team ON team_invites (team_id);
CREATE INDEX team_invites_by_invitee ON team_invites (invitee_id) WHERE status = 'pending';
-- one open invitation per team and user
CREATE UNIQUE INDEX team_invites_one_pending ON team_invites (team_id, invitee_id)
    WHERE kind = 'user' AND status = 'pending';

-- Sessions and laps use their own grants ('custom') or the next level's
-- ('inherit'): lap -> session -> the owner's default.
ALTER TABLE sessions ADD COLUMN sharing_mode text NOT NULL DEFAULT 'inherit'
    CHECK (sharing_mode IN ('inherit', 'custom'));
ALTER TABLE laps ADD COLUMN sharing_mode text NOT NULL DEFAULT 'inherit'
    CHECK (sharing_mode IN ('inherit', 'custom'));

-- Who may read what. object_id is the session id, the lap id, or the
-- owner's id for their default. grantee_id is a user or a team; the nil
-- UUID for "everyone on this server".
CREATE TABLE share_grants (
    owner_id      uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    object_kind   text NOT NULL CHECK (object_kind IN ('session', 'lap', 'user_default')),
    object_id     uuid NOT NULL,
    grantee_kind  text NOT NULL CHECK (grantee_kind IN ('user', 'team', 'everyone')),
    grantee_id    uuid NOT NULL DEFAULT '00000000-0000-0000-0000-000000000000',
    created_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (owner_id, object_kind, object_id, grantee_kind, grantee_id)
);
CREATE INDEX share_grants_by_grantee ON share_grants (grantee_kind, grantee_id);

CREATE INDEX laps_by_id ON laps (id);
