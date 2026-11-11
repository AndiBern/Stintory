// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sharing sessions and laps with users, teams and everyone on the server
//! (docs/PLAN.md §9.2).
//!
//! # Rules
//!
//! - A lap with mode `custom` uses its own grants; with `inherit` its
//!   session's. A session with `inherit` uses the owner's default grants,
//!   but only if the owner recorded it: imported sessions are private
//!   until shared explicitly.
//! - A team grant counts while both the viewer and the owner are members
//!   of the team (checked live).
//! - "Everyone" counts only while the admin allows it.
//! - Sharing gives read access to the session's basic data (track, car,
//!   conditions) and the laps with their telemetry; never to the setup or
//!   the game's session info, and never write access.
//!
//! # One access check
//!
//! [`lap_readable`] is the only place that decides who may read a lap.
//! Every endpoint that returns data of other users builds on it. A session
//! is visible to others through its readable laps.

use axum::Json;
use axum::extract::{Path, Query, State};
use deadpool_postgres::GenericClient;
use serde::{Deserialize, Serialize};
use sy_model::api::{
    ConditionsView, LapSummary, OriginView, REFERENCE_LAPS_MAX, REFERENCE_LAPS_PER_DRIVER,
    ReferenceLap, ReferenceScope, SessionDetail, SessionSummary,
};
use sy_model::remote::{
    ForeignLapCheck, ForeignLapCheckResult, SharedSession, SharedSessionDetail,
    SharedSessionDownload, SharedTrack, UserRef,
};
use sy_model::{LapId, Origin, Session, SessionId};
use tokio_postgres::Row;
use uuid::Uuid;

use crate::auth::{Auth, normalize_username};
use crate::data::{CLEAN_LAP, lap_from_row};
use crate::error::{ApiError, ApiResult};
use crate::state::{ServerSettings, SharedState};

/// Most grants per object.
const MAX_GRANTS: usize = 200;

/// SQL condition: the user `viewer` may read lap `l` of session `s`
/// (both aliases must be in scope). `viewer` is a placeholder for the
/// user's id (`$1`), `everyone` one for "sharing with everyone is allowed"
/// (`$2`, boolean).
pub fn lap_readable(viewer: &str, everyone: &str) -> String {
    format!(
        "(l.owner_id = {viewer} OR EXISTS (
            SELECT 1 FROM share_grants g
            WHERE g.owner_id = l.owner_id
              AND ((l.sharing_mode = 'custom' AND g.object_kind = 'lap' AND g.object_id = l.id)
                OR (l.sharing_mode = 'inherit' AND s.sharing_mode = 'custom'
                    AND g.object_kind = 'session' AND g.object_id = s.id)
                OR (l.sharing_mode = 'inherit' AND s.sharing_mode = 'inherit'
                    AND s.origin->>'kind' = 'own'
                    AND g.object_kind = 'user_default' AND g.object_id = l.owner_id))
              AND ((g.grantee_kind = 'user' AND g.grantee_id = {viewer})
                OR (g.grantee_kind = 'team'
                    AND EXISTS (SELECT 1 FROM team_members m
                                WHERE m.team_id = g.grantee_id AND m.user_id = {viewer})
                    AND EXISTS (SELECT 1 FROM team_members m
                                WHERE m.team_id = g.grantee_id AND m.user_id = l.owner_id))
                OR (g.grantee_kind = 'everyone' AND {everyone}))))"
    )
}

/// SQL condition: lap `l` (alias in scope) is readable by `viewer`; joins
/// the session itself.
pub fn lap_readable_with_session(viewer: &str, everyone: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM sessions s
                 WHERE s.owner_id = l.owner_id AND s.id = l.session_id AND {})",
        lap_readable(viewer, everyone)
    )
}

async fn everyone_allowed(db: &impl GenericClient) -> ApiResult<bool> {
    Ok(ServerSettings::load(db).await?.allow_share_everyone)
}

// ----- grants -----------------------------------------------------------

/// Who something is shared with, as sent by the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum GranteeRef {
    /// One user
    User {
        /// User id
        id: Uuid,
    },
    /// All members of a team
    Team {
        /// Team id
        id: Uuid,
    },
    /// Everyone with an account on this server
    Everyone,
}

/// Who something is shared with, with names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Grantee {
    /// One user
    User {
        /// User id
        id: Uuid,
        /// Login name
        username: String,
        /// Name shown to others
        display_name: String,
    },
    /// All members of a team
    Team {
        /// Team id
        id: Uuid,
        /// Team name
        name: String,
    },
    /// Everyone with an account on this server
    Everyone,
}

/// Whether an object uses its own grants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Uses the next level's grants (lap → session → default)
    Inherit,
    /// Uses its own grants
    Custom,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Custom => "custom",
        }
    }

    fn parse(s: &str) -> Self {
        if s == "custom" {
            Self::Custom
        } else {
            Self::Inherit
        }
    }
}

/// Where the effective audience of an object comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The object's own grants
    Own,
    /// The lap's session
    Session,
    /// The owner's default
    Default,
    /// Imported sessions don't use the default: private
    Imported,
}

/// Sharing of a session or lap: `GET/PUT /sessions/{id}/sharing`,
/// `/laps/{id}/sharing`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharingState {
    /// Own grants or inherited
    pub mode: Mode,
    /// The object's own grants (empty with `inherit`)
    pub grants: Vec<Grantee>,
    /// Where the effective audience comes from
    pub source: Source,
    /// Who can see it right now ("everyone" only while allowed)
    pub effective: Vec<Grantee>,
    /// Sharing with everyone is allowed on this server
    pub everyone_allowed: bool,
}

/// `PUT /sessions/{id}/sharing`, `/laps/{id}/sharing`
#[derive(Debug, Deserialize)]
pub struct SharingUpdate {
    /// Own grants or inherited
    pub mode: Mode,
    /// The new grants (ignored with `inherit`)
    #[serde(default)]
    pub grants: Vec<GranteeRef>,
}

async fn load_grants(
    db: &impl GenericClient,
    owner: Uuid,
    kind: &str,
    object: Uuid,
) -> ApiResult<Vec<Grantee>> {
    let rows = db
        .query(
            "SELECT g.grantee_kind, g.grantee_id, u.username, u.display_name, t.name
             FROM share_grants g
             LEFT JOIN users u ON g.grantee_kind = 'user' AND u.id = g.grantee_id
             LEFT JOIN teams t ON g.grantee_kind = 'team' AND t.id = g.grantee_id
             WHERE g.owner_id = $1 AND g.object_kind = $2 AND g.object_id = $3
             ORDER BY g.grantee_kind, lower(coalesce(t.name, u.display_name, '')), g.grantee_id",
            &[&owner, &kind, &object],
        )
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let id: Uuid = r.get(1);
            match r.get::<_, &str>(0) {
                "user" => Some(Grantee::User {
                    id,
                    username: r.get::<_, Option<String>>(2)?,
                    display_name: r.get::<_, Option<String>>(3)?,
                }),
                "team" => Some(Grantee::Team {
                    id,
                    name: r.get::<_, Option<String>>(4)?,
                }),
                _ => Some(Grantee::Everyone),
            }
        })
        .collect())
}

/// Replaces the grants of an object after checking them: users must exist,
/// teams must be the owner's, "everyone" must be allowed.
async fn replace_grants(
    db: &impl GenericClient,
    owner: Uuid,
    kind: &str,
    object: Uuid,
    grants: &[GranteeRef],
) -> ApiResult<()> {
    if grants.len() > MAX_GRANTS {
        return Err(ApiError::bad_request("too many people and teams"));
    }
    let mut seen = std::collections::HashSet::new();
    let grants: Vec<GranteeRef> = grants.iter().copied().filter(|g| seen.insert(*g)).collect();
    for g in &grants {
        match g {
            GranteeRef::User { id } => {
                let known = db
                    .query_opt("SELECT 1 FROM users WHERE id = $1 AND NOT disabled", &[id])
                    .await?
                    .is_some();
                if !known || *id == owner {
                    return Err(ApiError::bad_request("unknown user"));
                }
            }
            GranteeRef::Team { id } => {
                if crate::teams::role_of(db, *id, owner).await?.is_none() {
                    return Err(ApiError::bad_request(
                        "you can only share with teams you are a member of",
                    ));
                }
            }
            GranteeRef::Everyone => {
                if !everyone_allowed(db).await? {
                    return Err(ApiError::bad_request(
                        "sharing with everyone is turned off on this server",
                    ));
                }
            }
        }
    }
    db.execute(
        "DELETE FROM share_grants WHERE owner_id = $1 AND object_kind = $2 AND object_id = $3",
        &[&owner, &kind, &object],
    )
    .await?;
    for g in &grants {
        let (grantee_kind, id) = match g {
            GranteeRef::User { id } => ("user", *id),
            GranteeRef::Team { id } => ("team", *id),
            GranteeRef::Everyone => ("everyone", Uuid::nil()),
        };
        db.execute(
            "INSERT INTO share_grants (owner_id, object_kind, object_id, grantee_kind, grantee_id)
             VALUES ($1, $2, $3, $4, $5)",
            &[&owner, &kind, &object, &grantee_kind, &id],
        )
        .await?;
    }
    Ok(())
}

fn effective(grants: Vec<Grantee>, everyone: bool) -> Vec<Grantee> {
    grants
        .into_iter()
        .filter(|g| everyone || *g != Grantee::Everyone)
        .collect()
}

/// Mode and "recorded by the owner" of a session; 404 if it isn't the
/// user's.
async fn session_mode(db: &impl GenericClient, owner: Uuid, id: Uuid) -> ApiResult<(Mode, bool)> {
    let row = db
        .query_opt(
            "SELECT sharing_mode, origin->>'kind' = 'own' FROM sessions
             WHERE owner_id = $1 AND id = $2",
            &[&owner, &id],
        )
        .await?
        .ok_or_else(|| ApiError::not_found("session"))?;
    Ok((Mode::parse(row.get(0)), row.get(1)))
}

/// The grants a session passes on to laps that inherit, and their source.
async fn session_audience(
    db: &impl GenericClient,
    owner: Uuid,
    id: Uuid,
) -> ApiResult<(Mode, Source, Vec<Grantee>)> {
    Ok(match session_mode(db, owner, id).await? {
        (Mode::Custom, _) => (
            Mode::Custom,
            Source::Own,
            load_grants(db, owner, "session", id).await?,
        ),
        (Mode::Inherit, true) => (
            Mode::Inherit,
            Source::Default,
            load_grants(db, owner, "user_default", owner).await?,
        ),
        (Mode::Inherit, false) => (Mode::Inherit, Source::Imported, Vec::new()),
    })
}

async fn session_state(db: &impl GenericClient, owner: Uuid, id: Uuid) -> ApiResult<SharingState> {
    let everyone = everyone_allowed(db).await?;
    let (mode, source, audience) = session_audience(db, owner, id).await?;
    Ok(SharingState {
        mode,
        grants: if mode == Mode::Custom {
            audience.clone()
        } else {
            Vec::new()
        },
        source,
        effective: effective(audience, everyone),
        everyone_allowed: everyone,
    })
}

async fn lap_state(db: &impl GenericClient, owner: Uuid, id: Uuid) -> ApiResult<SharingState> {
    let everyone = everyone_allowed(db).await?;
    let row = db
        .query_opt(
            "SELECT sharing_mode, session_id FROM laps WHERE owner_id = $1 AND id = $2",
            &[&owner, &id],
        )
        .await?
        .ok_or_else(|| ApiError::not_found("lap"))?;
    let mode = Mode::parse(row.get(0));
    let (grants, source, audience) = if mode == Mode::Custom {
        let grants = load_grants(db, owner, "lap", id).await?;
        (grants.clone(), Source::Own, grants)
    } else {
        let (session_mode, source, audience) = session_audience(db, owner, row.get(1)).await?;
        let source = if session_mode == Mode::Custom {
            Source::Session
        } else {
            source
        };
        (Vec::new(), source, audience)
    };
    Ok(SharingState {
        mode,
        grants,
        source,
        effective: effective(audience, everyone),
        everyone_allowed: everyone,
    })
}

/// `GET /sessions/{id}/sharing`
pub async fn get_session_sharing(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<SharingState>> {
    let db = state.conn().await?;
    Ok(Json(session_state(&db, session.user.id, id).await?))
}

/// `PUT /sessions/{id}/sharing`
pub async fn put_session_sharing(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Path(id): Path<Uuid>,
    Json(req): Json<SharingUpdate>,
) -> ApiResult<Json<SharingState>> {
    let owner = session.user.id;
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    let n = tx
        .execute(
            "UPDATE sessions SET sharing_mode = $3 WHERE owner_id = $1 AND id = $2",
            &[&owner, &id, &req.mode.as_str()],
        )
        .await?;
    if n == 0 {
        return Err(ApiError::not_found("session"));
    }
    let grants = if req.mode == Mode::Custom {
        req.grants
    } else {
        Vec::new()
    };
    replace_grants(&tx, owner, "session", id, &grants).await?;
    let out = session_state(&tx, owner, id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// `GET /laps/{id}/sharing`
pub async fn get_lap_sharing(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<SharingState>> {
    let db = state.conn().await?;
    Ok(Json(lap_state(&db, session.user.id, id).await?))
}

/// `PUT /laps/{id}/sharing`
pub async fn put_lap_sharing(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Path(id): Path<Uuid>,
    Json(req): Json<SharingUpdate>,
) -> ApiResult<Json<SharingState>> {
    let owner = session.user.id;
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    let n = tx
        .execute(
            "UPDATE laps SET sharing_mode = $3 WHERE owner_id = $1 AND id = $2",
            &[&owner, &id, &req.mode.as_str()],
        )
        .await?;
    if n == 0 {
        return Err(ApiError::not_found("lap"));
    }
    let grants = if req.mode == Mode::Custom {
        req.grants
    } else {
        Vec::new()
    };
    replace_grants(&tx, owner, "lap", id, &grants).await?;
    let out = lap_state(&tx, owner, id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// `GET/PUT /me/default-sharing`
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultSharing {
    /// Who sees the user's sessions unless a session or lap says otherwise
    pub grants: Vec<Grantee>,
    /// Sharing with everyone is allowed on this server
    pub everyone_allowed: bool,
    /// Sessions that use the default (a change applies to them)
    pub inheriting_sessions: i64,
}

async fn default_state(db: &impl GenericClient, owner: Uuid) -> ApiResult<DefaultSharing> {
    Ok(DefaultSharing {
        grants: load_grants(db, owner, "user_default", owner).await?,
        everyone_allowed: everyone_allowed(db).await?,
        inheriting_sessions: db
            .query_one(
                "SELECT count(*) FROM sessions
                 WHERE owner_id = $1 AND sharing_mode = 'inherit' AND origin->>'kind' = 'own'",
                &[&owner],
            )
            .await?
            .get(0),
    })
}

/// The user's default sharing.
pub async fn get_default(
    State(state): State<SharedState>,
    Auth(session): Auth,
) -> ApiResult<Json<DefaultSharing>> {
    let db = state.conn().await?;
    Ok(Json(default_state(&db, session.user.id).await?))
}

/// `PUT /me/default-sharing`
#[derive(Debug, Deserialize)]
pub struct DefaultUpdate {
    /// The new default grants
    pub grants: Vec<GranteeRef>,
}

/// Sets the user's default sharing; applies to every session and lap that
/// inherits.
pub async fn put_default(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Json(req): Json<DefaultUpdate>,
) -> ApiResult<Json<DefaultSharing>> {
    let owner = session.user.id;
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    replace_grants(&tx, owner, "user_default", owner, &req.grants).await?;
    let out = default_state(&tx, owner).await?;
    tx.commit().await?;
    Ok(Json(out))
}

// ----- finding users ------------------------------------------------------

/// `GET /users/lookup?username=`
#[derive(Debug, Deserialize)]
pub struct LookupQuery {
    /// Exact username
    pub username: String,
}

/// Finds a user by exact username. There is no user directory: names
/// can't be listed.
pub async fn lookup_user(
    State(state): State<SharedState>,
    _: Auth,
    Query(q): Query<LookupQuery>,
) -> ApiResult<Json<UserRef>> {
    let not_found = || ApiError::not_found("a user with this name");
    let username = normalize_username(&q.username).map_err(|_| not_found())?;
    let row = state
        .conn()
        .await?
        .query_opt(
            "SELECT id, username, display_name FROM users WHERE username = $1 AND NOT disabled",
            &[&username],
        )
        .await?
        .ok_or_else(not_found)?;
    Ok(Json(UserRef {
        id: row.get(0),
        username: row.get(1),
        display_name: row.get(2),
    }))
}

/// `GET /users/suggest?q=`
#[derive(Debug, Deserialize)]
pub struct SuggestQuery {
    /// Part of a username or display name
    #[serde(default)]
    pub q: String,
}

/// Suggests people the user shares a team with.
pub async fn suggest_users(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Query(q): Query<SuggestQuery>,
) -> ApiResult<Json<Vec<UserRef>>> {
    let rows = state
        .conn()
        .await?
        .query(
            "SELECT DISTINCT u.id, u.username, u.display_name
             FROM team_members mine
             JOIN team_members other ON other.team_id = mine.team_id
             JOIN users u ON u.id = other.user_id
             WHERE mine.user_id = $1 AND u.id <> $1 AND NOT u.disabled
               AND (strpos(u.username, lower($2)) > 0
                    OR strpos(lower(u.display_name), lower($2)) > 0)
             ORDER BY u.username LIMIT 20",
            &[&session.user.id, &q.q.trim()],
        )
        .await?;
    Ok(Json(
        rows.iter()
            .map(|r| UserRef {
                id: r.get(0),
                username: r.get(1),
                display_name: r.get(2),
            })
            .collect(),
    ))
}

// ----- shared with me -----------------------------------------------------

/// Sessions of others with at least one lap the user may read (optionally
/// one session only). Lap count and best lap cover the readable laps.
async fn shared_sessions(
    state: &SharedState,
    db: &impl GenericClient,
    viewer: Uuid,
    only: Option<(Uuid, Uuid)>,
) -> ApiResult<Vec<SharedSession>> {
    let everyone = everyone_allowed(db).await?;
    let (owner, id) = only.unzip();
    let rows = db
        .query(
            &format!(
                "SELECT s.id, s.game, s.track_key, s.car_key, s.session_type, s.started_at,
                        s.track_name, s.car_name, count(l.id),
                        min(CASE WHEN {CLEAN_LAP} THEN l.lap_time_ms END),
                        u.id, u.username, u.display_name
                 FROM sessions s
                 JOIN laps l ON l.owner_id = s.owner_id AND l.session_id = s.id
                 JOIN users u ON u.id = s.owner_id
                 WHERE s.owner_id <> $1 AND {readable}
                   AND ($3::uuid IS NULL OR (s.owner_id = $3 AND s.id = $4))
                 GROUP BY s.owner_id, s.id, u.id
                 ORDER BY s.started_at DESC
                 LIMIT 1000",
                readable = lap_readable("$1", "$2")
            ),
            &[&viewer, &everyone, &owner, &id],
        )
        .await?;
    rows.iter().map(|r| shared_from_row(state, r)).collect()
}

fn bad<T: std::str::FromStr>(value: &str, column: &str) -> ApiResult<T> {
    value
        .parse()
        .map_err(|_| ApiError::internal(format!("bad value in {column}: {value:?}")))
}

fn shared_from_row(state: &SharedState, r: &Row) -> ApiResult<SharedSession> {
    let owner = UserRef {
        id: r.get(10),
        username: r.get(11),
        display_name: r.get(12),
    };
    Ok(SharedSession {
        summary: SessionSummary {
            id: SessionId::from_uuid(r.get(0)),
            game: bad(r.get(1), "sessions.game")?,
            track_key: r.get(2),
            car_key: r.get(3),
            session_type: bad(r.get(4), "sessions.session_type")?,
            started_at: r.get(5),
            track_name: r.get(6),
            car_name: r.get(7),
            lap_count: u32::try_from(r.get::<_, i64>(8)).unwrap_or(0),
            best_lap_ms: r.get::<_, Option<i32>>(9).map(|v| v as u32),
            origin: OriginView::Synced {
                server: state.config.public_url.clone(),
                owner: owner.username.clone(),
            },
            shared: false,
        },
        owner,
    })
}

/// `GET /shared/sessions`: sessions others shared with the user, newest
/// first.
pub async fn list_shared(
    State(state): State<SharedState>,
    Auth(session): Auth,
) -> ApiResult<Json<Vec<SharedSession>>> {
    let db = state.conn().await?;
    Ok(Json(
        shared_sessions(&state, &db, session.user.id, None).await?,
    ))
}

/// The laps of a session of `owner` that `viewer` may read.
async fn readable_laps(
    db: &impl GenericClient,
    viewer: Uuid,
    owner: Uuid,
    session: Uuid,
) -> ApiResult<Vec<sy_model::Lap>> {
    let everyone = everyone_allowed(db).await?;
    let rows = db
        .query(
            &format!(
                "SELECT {columns} FROM laps l
                 WHERE l.owner_id = $3 AND l.session_id = $4 AND {readable}
                 ORDER BY l.number, l.id",
                columns = crate::data::LAP_COLUMNS,
                readable = lap_readable_with_session("$1", "$2")
            ),
            &[&viewer, &everyone, &owner, &session],
        )
        .await?;
    rows.iter().map(lap_from_row).collect()
}

/// `GET /shared/sessions/{owner}/{id}`: a shared session with the laps the
/// user may see.
pub async fn get_shared(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Path((owner, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<SharedSessionDetail>> {
    let db = state.conn().await?;
    let viewer = session.user.id;
    let shared = shared_sessions(&state, &db, viewer, Some((owner, id)))
        .await?
        .pop()
        .ok_or_else(|| ApiError::not_found("session"))?;
    let row = db
        .query_one(
            "SELECT ended_at, game_version, conditions FROM sessions
             WHERE owner_id = $1 AND id = $2",
            &[&owner, &id],
        )
        .await?;
    let conditions: sy_model::session::Conditions = serde_json::from_value(row.get(2))?;
    let laps = readable_laps(&db, viewer, owner, id).await?;
    Ok(Json(SharedSessionDetail {
        detail: SessionDetail {
            summary: shared.summary,
            ended_at: row.get(0),
            game_version: row.get(1),
            conditions: ConditionsView::from(&conditions),
            laps: laps.iter().map(LapSummary::from).collect(),
        },
        owner: shared.owner,
    }))
}

/// `GET /shared/sessions/{owner}/{id}/download`: the same for a desktop
/// app that keeps the laps as a reference. Without the setup and the
/// game's session info.
pub async fn download_shared(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Path((owner, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<SharedSessionDownload>> {
    let db = state.conn().await?;
    let viewer = session.user.id;
    let shared = shared_sessions(&state, &db, viewer, Some((owner, id)))
        .await?
        .pop()
        .ok_or_else(|| ApiError::not_found("session"))?;
    let row = db
        .query_one(
            "SELECT ended_at, game_version, conditions FROM sessions
             WHERE owner_id = $1 AND id = $2",
            &[&owner, &id],
        )
        .await?;
    let s = shared.summary;
    Ok(Json(SharedSessionDownload {
        session: Session {
            id: s.id,
            game: s.game,
            game_version: row.get(1),
            track_key: s.track_key,
            track_name: s.track_name,
            car_key: s.car_key,
            car_name: s.car_name,
            session_type: s.session_type,
            started_at: s.started_at,
            ended_at: row.get(0),
            conditions: serde_json::from_value(row.get(2))?,
            setup: None,
            session_info: None,
            origin: Origin::Synced {
                server: state.config.public_url.clone(),
                owner: shared.owner.username.clone(),
            },
        },
        laps: readable_laps(&db, viewer, owner, id).await?,
        owner: shared.owner,
    }))
}

/// `POST /shared/check`: which of the laps of others a client keeps the
/// user may not read anymore (docs/PLAN.md §9.2, revocation).
pub async fn check_foreign(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Json(req): Json<ForeignLapCheck>,
) -> ApiResult<Json<ForeignLapCheckResult>> {
    if req.laps.len() > 20_000 {
        return Err(ApiError::bad_request("too many laps, send them in batches"));
    }
    let db = state.conn().await?;
    let everyone = everyone_allowed(&db).await?;
    let ids: Vec<Uuid> = req.laps.iter().map(|l| *l.id.as_uuid()).collect();
    let readable: std::collections::HashSet<(Uuid, String)> = db
        .query(
            &format!(
                "SELECT l.id, u.username FROM laps l JOIN users u ON u.id = l.owner_id
                 WHERE l.id = ANY($3) AND l.owner_id <> $1 AND {}",
                lap_readable_with_session("$1", "$2")
            ),
            &[&session.user.id, &everyone, &ids],
        )
        .await?
        .iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect();
    Ok(Json(ForeignLapCheckResult {
        lost: req
            .laps
            .into_iter()
            .filter(|l| !readable.contains(&(*l.id.as_uuid(), l.owner.clone())))
            .map(|l| l.id)
            .collect(),
    }))
}

/// A lap the user may read, with who drove it.
#[derive(Debug, Clone)]
pub struct ReadableLap {
    /// The lap
    pub lap: sy_model::Lap,
    /// The owner, if it isn't the user's own lap
    pub owner: Option<UserRef>,
    /// Origin of the lap's session (as stored for its owner)
    pub origin: Origin,
}

impl ReadableLap {
    /// Who drove the lap if not the user (see [`sy_model::api::LapTrace`]).
    pub fn driver(&self) -> Option<String> {
        match (&self.owner, &self.origin) {
            (Some(owner), _) if owner.display_name.is_empty() => Some(owner.username.clone()),
            (Some(owner), _) => Some(owner.display_name.clone()),
            (None, Origin::Imported { driver_name }) => driver_name.clone(),
            (None, _) => None,
        }
    }
}

/// A lap the user may read by id: their own, else one shared with them.
pub async fn readable_lap(
    db: &impl GenericClient,
    viewer: Uuid,
    id: Uuid,
) -> ApiResult<Option<ReadableLap>> {
    let everyone = everyone_allowed(db).await?;
    let columns = crate::data::LAP_COLUMNS
        .split(", ")
        .map(|c| format!("l.{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let Some(r) = db
        .query_opt(
            &format!(
                "SELECT {columns}, u.id, u.username, u.display_name, s.origin
                 FROM laps l
                 JOIN sessions s ON s.owner_id = l.owner_id AND s.id = l.session_id
                 JOIN users u ON u.id = l.owner_id
                 WHERE l.id = $3 AND {readable}
                 ORDER BY (l.owner_id = $1) DESC, l.owner_id LIMIT 1",
                readable = lap_readable("$1", "$2")
            ),
            &[&viewer, &everyone, &id],
        )
        .await?
    else {
        return Ok(None);
    };
    let owner_id: Uuid = r.get(12);
    Ok(Some(ReadableLap {
        lap: lap_from_row(&r)?,
        owner: (owner_id != viewer).then(|| UserRef {
            id: owner_id,
            username: r.get(13),
            display_name: r.get(14),
        }),
        origin: serde_json::from_value(r.get(15))?,
    }))
}

/// True if the user may read a lap with this telemetry file.
pub async fn blob_readable(db: &impl GenericClient, viewer: Uuid, hash: &str) -> ApiResult<bool> {
    let everyone = everyone_allowed(db).await?;
    Ok(db
        .query_opt(
            &format!(
                "SELECT 1 FROM laps l WHERE l.blob_hash = $3 AND {} LIMIT 1",
                lap_readable_with_session("$1", "$2")
            ),
            &[&viewer, &everyone, &hash],
        )
        .await?
        .is_some())
}

// ----- reference laps by track and car -------------------------------------

/// `GET /shared/tracks`: tracks and cars with laps others shared with the
/// user, most recently driven first.
pub async fn shared_tracks(
    State(state): State<SharedState>,
    Auth(session): Auth,
) -> ApiResult<Json<Vec<SharedTrack>>> {
    let db = state.conn().await?;
    let everyone = everyone_allowed(&db).await?;
    let rows = db
        .query(
            &format!(
                "WITH shared AS (
                    SELECT s.game, s.track_key, s.track_name, s.car_key, s.car_name,
                           s.started_at, l.owner_id,
                           CASE WHEN {CLEAN_LAP} THEN l.lap_time_ms END AS clean_ms
                    FROM laps l
                    JOIN sessions s ON s.owner_id = l.owner_id AND s.id = l.session_id
                    WHERE l.owner_id <> $1 AND {readable}
                 ), tracks AS (
                    SELECT game, track_key, max(track_name) AS track_name,
                           car_key, max(car_name) AS car_name,
                           count(clean_ms) AS laps, count(DISTINCT owner_id) AS drivers,
                           min(clean_ms) AS best_ms,
                           (array_agg(owner_id ORDER BY clean_ms)
                               FILTER (WHERE clean_ms IS NOT NULL))[1] AS best_owner,
                           max(started_at) AS latest
                    FROM shared GROUP BY game, track_key, car_key
                 )
                 SELECT t.game, t.track_key, t.track_name, t.car_key, t.car_name, t.laps,
                        t.drivers, t.best_ms, u.id, u.username, u.display_name,
                        (SELECT min(l.lap_time_ms) FROM laps l
                         JOIN sessions s ON s.owner_id = l.owner_id AND s.id = l.session_id
                         WHERE l.owner_id = $1 AND s.track_key = t.track_key
                           AND s.car_key = t.car_key AND {CLEAN_LAP}),
                        t.latest
                 FROM tracks t LEFT JOIN users u ON u.id = t.best_owner
                 ORDER BY t.latest DESC, t.track_key, t.car_key
                 LIMIT 500",
                readable = lap_readable("$1", "$2")
            ),
            &[&session.user.id, &everyone],
        )
        .await?;
    let tracks = rows
        .iter()
        .map(|r| {
            Ok(SharedTrack {
                game: bad(r.get(0), "sessions.game")?,
                track_key: r.get(1),
                track_name: r.get(2),
                car_key: r.get(3),
                car_name: r.get(4),
                laps: u32::try_from(r.get::<_, i64>(5)).unwrap_or(0),
                drivers: u32::try_from(r.get::<_, i64>(6)).unwrap_or(0),
                best_lap_ms: r.get::<_, Option<i32>>(7).map(|v| v as u32),
                best_driver: r.get::<_, Option<Uuid>>(8).map(|id| UserRef {
                    id,
                    username: r.get(9),
                    display_name: r.get(10),
                }),
                own_best_lap_ms: r.get::<_, Option<i32>>(11).map(|v| v as u32),
                latest_at: r.get(12),
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(tracks))
}

/// `GET /reference-laps?track=&car=&scope=`
#[derive(Debug, Deserialize)]
pub struct ReferenceQuery {
    /// Canonical track key
    pub track: String,
    /// Canonical car key
    pub car: String,
    /// Own laps, shared laps or both
    #[serde(default)]
    pub scope: ReferenceScope,
}

/// Laps to compare with for one track and car, fastest first: the user's
/// own (recorded and imported) and those others shared with them. Only
/// laps that count for a best lap, at most [`REFERENCE_LAPS_PER_DRIVER`]
/// per driver.
pub async fn reference_laps(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Query(q): Query<ReferenceQuery>,
) -> ApiResult<Json<Vec<ReferenceLap>>> {
    let db = state.conn().await?;
    let viewer = session.user.id;
    let everyone = everyone_allowed(&db).await?;
    let mine = q.scope != ReferenceScope::Shared;
    let shared = q.scope != ReferenceScope::Mine;
    let rows = db
        .query(
            &format!(
                "WITH candidates AS (
                    SELECT l.id, l.session_id, l.number, l.lap_time_ms, l.sector_times_ms,
                           s.game, s.track_key, s.track_name, s.car_key, s.car_name,
                           s.session_type, s.started_at, s.origin, l.owner_id,
                           row_number() OVER (
                               PARTITION BY l.owner_id, s.origin->>'kind',
                                            coalesce(s.origin->>'driver_name', '')
                               ORDER BY l.lap_time_ms, s.started_at, l.id) AS rank
                    FROM laps l
                    JOIN sessions s ON s.owner_id = l.owner_id AND s.id = l.session_id
                    WHERE s.track_key = $3 AND s.car_key = $4
                      AND l.lap_time_ms IS NOT NULL AND {CLEAN_LAP}
                      AND (($5 AND l.owner_id = $1)
                           OR ($6 AND l.owner_id <> $1 AND {readable}))
                 )
                 SELECT c.id, c.session_id, c.number, c.lap_time_ms, c.sector_times_ms,
                        c.game, c.track_key, c.track_name, c.car_key, c.car_name,
                        c.session_type, c.started_at, c.origin,
                        u.id, u.username, u.display_name
                 FROM candidates c JOIN users u ON u.id = c.owner_id
                 WHERE c.rank <= $7
                 ORDER BY c.lap_time_ms, c.started_at, c.id
                 LIMIT $8",
                readable = lap_readable("$1", "$2")
            ),
            &[
                &viewer,
                &everyone,
                &q.track,
                &q.car,
                &mine,
                &shared,
                &(REFERENCE_LAPS_PER_DRIVER as i64),
                &(REFERENCE_LAPS_MAX as i64),
            ],
        )
        .await?;
    let laps = rows
        .iter()
        .map(|r| {
            let owner_id: Uuid = r.get(13);
            let owner = (owner_id != viewer).then(|| UserRef {
                id: owner_id,
                username: r.get(14),
                display_name: r.get(15),
            });
            let origin = match &owner {
                Some(o) => OriginView::Synced {
                    server: state.config.public_url.clone(),
                    owner: o.username.clone(),
                },
                None => serde_json::from_value::<Origin>(r.get(12))?.into(),
            };
            Ok(ReferenceLap {
                lap_id: LapId::from_uuid(r.get(0)),
                session_id: SessionId::from_uuid(r.get(1)),
                number: u32::try_from(r.get::<_, i32>(2)).unwrap_or(0),
                lap_time_ms: u32::try_from(r.get::<_, i32>(3)).unwrap_or(0),
                sector_times_ms: serde_json::from_value(r.get(4))?,
                game: bad(r.get(5), "sessions.game")?,
                track_key: r.get(6),
                track_name: r.get(7),
                car_key: r.get(8),
                car_name: r.get(9),
                session_type: bad(r.get(10), "sessions.session_type")?,
                session_started_at: r.get(11),
                origin,
                owner,
                remote: false,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(laps))
}
