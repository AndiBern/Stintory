// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Teams, roles and invitations (docs/PLAN.md §9.2).
//!
//! - Whoever creates a team is its owner. Owners and admins invite, remove
//!   members and change roles; admins can't touch owners or make owners.
//!   Owners rename and delete the team. A team always has an owner.
//! - Invitations go to a user (who accepts or declines) or are a link
//!   (random token, stored hashed, with expiry and an optional number of
//!   uses) for anyone logged in.
//! - A team someone isn't a member of looks like it doesn't exist (404).
//!
//! Changes of one team are serialized with a row lock on the team.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use deadpool_postgres::GenericClient;
use serde::{Deserialize, Serialize};
use sy_model::remote::UserRef;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::{Auth, ClientIp, normalize_username};
use crate::crypto;
use crate::error::{ApiError, ApiResult};
use crate::state::{SharedState, audit};

/// Longest team name, in characters.
const MAX_NAME: usize = 64;
/// Default and longest life of an invite link, in days.
const LINK_DAYS: (i32, i32) = (7, 365);

/// What a member may do in a team.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Sees what is shared with the team
    Member,
    /// Also invites, removes members and changes roles (not of owners)
    Admin,
    /// Also renames and deletes the team and appoints owners
    Owner,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Admin => "admin",
            Self::Owner => "owner",
        }
    }

    fn parse(s: &str) -> ApiResult<Self> {
        match s {
            "member" => Ok(Self::Member),
            "admin" => Ok(Self::Admin),
            "owner" => Ok(Self::Owner),
            _ => Err(ApiError::internal(format!("bad team role {s:?}"))),
        }
    }
}

/// A team the user is a member of.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamSummary {
    /// Id
    pub id: Uuid,
    /// Name
    pub name: String,
    /// The user's role
    pub role: Role,
    /// Number of members
    pub member_count: i64,
    /// Created
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// A member of a team.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    /// Who
    pub user: UserRef,
    /// Role
    pub role: Role,
    /// Member since
    #[serde(with = "time::serde::rfc3339")]
    pub joined_at: OffsetDateTime,
}

/// An open invitation of a team.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Invite {
    /// Id
    pub id: Uuid,
    /// `user` or `link`
    pub kind: String,
    /// The invited user (kind `user`)
    pub invitee: Option<UserRef>,
    /// Who invited
    pub invited_by: Option<UserRef>,
    /// The link stops working then (kind `link`)
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    /// How often the link can be used; None = unlimited
    pub max_uses: Option<i32>,
    /// How often the link was used
    pub uses: i32,
    /// Created
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// The link (only in the answer that creates it; shown once)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// `GET /teams/{id}`
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamDetail {
    /// The team
    #[serde(flatten)]
    pub team: TeamSummary,
    /// Members, owners first
    pub members: Vec<Member>,
    /// Open invitations (owners and admins only)
    pub invites: Vec<Invite>,
}

fn user_ref(
    id: Option<Uuid>,
    username: Option<String>,
    display: Option<String>,
) -> Option<UserRef> {
    Some(UserRef {
        id: id?,
        username: username?,
        display_name: display?,
    })
}

fn team_name(name: &str) -> ApiResult<String> {
    let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ");
    match name.chars().count() {
        0 => Err(ApiError::bad_request("the team needs a name")),
        n if n > MAX_NAME => Err(ApiError::bad_request("the team name is too long")),
        _ => Ok(name),
    }
}

/// The user's role in a team, if they are a member.
pub async fn role_of(db: &impl GenericClient, team: Uuid, user: Uuid) -> ApiResult<Option<Role>> {
    db.query_opt(
        "SELECT role FROM team_members WHERE team_id = $1 AND user_id = $2",
        &[&team, &user],
    )
    .await?
    .map(|r| Role::parse(r.get(0)))
    .transpose()
}

/// The member's role; 404 for non-members (the team stays hidden).
async fn member_role(db: &impl GenericClient, team: Uuid, user: Uuid) -> ApiResult<Role> {
    role_of(db, team, user)
        .await?
        .ok_or_else(|| ApiError::not_found("team"))
}

/// Like [`member_role`], but owners and admins only.
async fn manager_role(db: &impl GenericClient, team: Uuid, user: Uuid) -> ApiResult<Role> {
    match member_role(db, team, user).await? {
        Role::Member => Err(ApiError::forbidden(
            "only owners and admins of the team can do this",
        )),
        role => Ok(role),
    }
}

/// Serializes changes of a team until the transaction ends.
async fn lock_team(db: &impl GenericClient, team: Uuid) -> ApiResult<()> {
    db.query_opt("SELECT 1 FROM teams WHERE id = $1 FOR UPDATE", &[&team])
        .await?
        .map(|_| ())
        .ok_or_else(|| ApiError::not_found("team"))
}

async fn owner_count(db: &impl GenericClient, team: Uuid) -> ApiResult<i64> {
    Ok(db
        .query_one(
            "SELECT count(*) FROM team_members WHERE team_id = $1 AND role = 'owner'",
            &[&team],
        )
        .await?
        .get(0))
}

async fn summary(db: &impl GenericClient, team: Uuid, user: Uuid) -> ApiResult<TeamSummary> {
    let row = db
        .query_opt(
            "SELECT t.id, t.name, m.role, t.created_at,
                    (SELECT count(*) FROM team_members x WHERE x.team_id = t.id)
             FROM teams t JOIN team_members m ON m.team_id = t.id AND m.user_id = $2
             WHERE t.id = $1",
            &[&team, &user],
        )
        .await?
        .ok_or_else(|| ApiError::not_found("team"))?;
    summary_from_row(&row)
}

fn summary_from_row(r: &tokio_postgres::Row) -> ApiResult<TeamSummary> {
    Ok(TeamSummary {
        id: r.get(0),
        name: r.get(1),
        role: Role::parse(r.get(2))?,
        created_at: r.get(3),
        member_count: r.get(4),
    })
}

/// What a user shared with a team stops when they are no longer in it.
async fn drop_grants_of(db: &impl GenericClient, team: Uuid, owner: Uuid) -> ApiResult<()> {
    db.execute(
        "DELETE FROM share_grants
         WHERE owner_id = $2 AND grantee_kind = 'team' AND grantee_id = $1",
        &[&team, &owner],
    )
    .await?;
    Ok(())
}

async fn delete_team(db: &impl GenericClient, team: Uuid) -> ApiResult<()> {
    db.execute(
        "DELETE FROM share_grants WHERE grantee_kind = 'team' AND grantee_id = $1",
        &[&team],
    )
    .await?;
    db.execute("DELETE FROM teams WHERE id = $1", &[&team])
        .await?;
    Ok(())
}

// ----- teams ------------------------------------------------------------

/// `GET /teams`: the user's teams.
pub async fn list(
    State(state): State<SharedState>,
    Auth(session): Auth,
) -> ApiResult<Json<Vec<TeamSummary>>> {
    let rows = state
        .conn()
        .await?
        .query(
            "SELECT t.id, t.name, m.role, t.created_at,
                    (SELECT count(*) FROM team_members x WHERE x.team_id = t.id)
             FROM teams t JOIN team_members m ON m.team_id = t.id AND m.user_id = $1
             ORDER BY lower(t.name), t.id",
            &[&session.user.id],
        )
        .await?;
    Ok(Json(
        rows.iter()
            .map(summary_from_row)
            .collect::<ApiResult<_>>()?,
    ))
}

/// `POST /teams`, `PATCH /teams/{id}`
#[derive(Debug, Deserialize)]
pub struct TeamName {
    /// Name of the team
    pub name: String,
}

/// Creates a team; the user becomes its owner.
pub async fn create(
    State(state): State<SharedState>,
    Auth(session): Auth,
    ip: ClientIp,
    Json(req): Json<TeamName>,
) -> ApiResult<Json<TeamSummary>> {
    let name = team_name(&req.name)?;
    let id = Uuid::now_v7();
    let user = session.user.id;
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    tx.execute(
        "INSERT INTO teams (id, name, created_by) VALUES ($1, $2, $3)",
        &[&id, &name, &user],
    )
    .await?;
    tx.execute(
        "INSERT INTO team_members (team_id, user_id, role) VALUES ($1, $2, 'owner')",
        &[&id, &user],
    )
    .await?;
    audit(
        &tx,
        Some(user),
        "team_created",
        ip.as_deref(),
        serde_json::json!({ "team": id, "name": name }),
    )
    .await;
    let team = summary(&tx, id, user).await?;
    tx.commit().await?;
    Ok(Json(team))
}

/// `GET /teams/{id}`
pub async fn get(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<TeamDetail>> {
    let db = state.conn().await?;
    let team = summary(&db, id, session.user.id).await?;
    let members = db
        .query(
            "SELECT u.id, u.username, u.display_name, m.role, m.joined_at
             FROM team_members m JOIN users u ON u.id = m.user_id
             WHERE m.team_id = $1
             ORDER BY CASE m.role WHEN 'owner' THEN 0 WHEN 'admin' THEN 1 ELSE 2 END,
                      lower(u.display_name), u.username",
            &[&id],
        )
        .await?
        .iter()
        .map(|r| {
            Ok(Member {
                user: UserRef {
                    id: r.get(0),
                    username: r.get(1),
                    display_name: r.get(2),
                },
                role: Role::parse(r.get(3))?,
                joined_at: r.get(4),
            })
        })
        .collect::<ApiResult<_>>()?;
    let invites = if team.role >= Role::Admin {
        open_invites(&db, id, None).await?
    } else {
        Vec::new()
    };
    Ok(Json(TeamDetail {
        team,
        members,
        invites,
    }))
}

/// `PATCH /teams/{id}`: renames the team (owners).
pub async fn rename(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Path(id): Path<Uuid>,
    Json(req): Json<TeamName>,
) -> ApiResult<Json<TeamSummary>> {
    let name = team_name(&req.name)?;
    let db = state.conn().await?;
    if member_role(&db, id, session.user.id).await? != Role::Owner {
        return Err(ApiError::forbidden("only owners can rename the team"));
    }
    db.execute("UPDATE teams SET name = $2 WHERE id = $1", &[&id, &name])
        .await?;
    Ok(Json(summary(&db, id, session.user.id).await?))
}

/// `DELETE /teams/{id}`: deletes the team (owners). What was shared with
/// it isn't shared anymore.
pub async fn delete(
    State(state): State<SharedState>,
    Auth(session): Auth,
    ip: ClientIp,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    if member_role(&tx, id, session.user.id).await? != Role::Owner {
        return Err(ApiError::forbidden("only owners can delete the team"));
    }
    lock_team(&tx, id).await?;
    delete_team(&tx, id).await?;
    audit(
        &tx,
        Some(session.user.id),
        "team_deleted",
        ip.as_deref(),
        serde_json::json!({ "team": id }),
    )
    .await;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ----- members ----------------------------------------------------------

/// `PATCH /teams/{id}/members/{user}`
#[derive(Debug, Deserialize)]
pub struct MemberUpdate {
    /// New role
    pub role: Role,
}

/// Changes a member's role.
pub async fn update_member(
    State(state): State<SharedState>,
    Auth(session): Auth,
    ip: ClientIp,
    Path((id, user)): Path<(Uuid, Uuid)>,
    Json(req): Json<MemberUpdate>,
) -> ApiResult<StatusCode> {
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    let actor = manager_role(&tx, id, session.user.id).await?;
    lock_team(&tx, id).await?;
    let target = role_of(&tx, id, user)
        .await?
        .ok_or_else(|| ApiError::not_found("member"))?;
    if actor != Role::Owner && (target == Role::Owner || req.role == Role::Owner) {
        return Err(ApiError::forbidden(
            "only owners can change owners or make someone an owner",
        ));
    }
    if target == Role::Owner && req.role != Role::Owner && owner_count(&tx, id).await? <= 1 {
        return Err(ApiError::conflict(
            "a team needs an owner: make someone else an owner first",
        ));
    }
    tx.execute(
        "UPDATE team_members SET role = $3 WHERE team_id = $1 AND user_id = $2",
        &[&id, &user, &req.role.as_str()],
    )
    .await?;
    audit(
        &tx,
        Some(session.user.id),
        "team_role_changed",
        ip.as_deref(),
        serde_json::json!({ "team": id, "user": user, "role": req.role.as_str() }),
    )
    .await;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /teams/{id}/members/{user}`: removes a member. They lose access
/// to what is shared with the team, and what they shared with it stops.
pub async fn remove_member(
    State(state): State<SharedState>,
    Auth(session): Auth,
    ip: ClientIp,
    Path((id, user)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    if user == session.user.id {
        return Err(ApiError::bad_request("to leave a team, use \"leave\""));
    }
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    let actor = manager_role(&tx, id, session.user.id).await?;
    lock_team(&tx, id).await?;
    let target = role_of(&tx, id, user)
        .await?
        .ok_or_else(|| ApiError::not_found("member"))?;
    if target == Role::Owner && actor != Role::Owner {
        return Err(ApiError::forbidden("only owners can remove owners"));
    }
    tx.execute(
        "DELETE FROM team_members WHERE team_id = $1 AND user_id = $2",
        &[&id, &user],
    )
    .await?;
    drop_grants_of(&tx, id, user).await?;
    audit(
        &tx,
        Some(session.user.id),
        "team_member_removed",
        ip.as_deref(),
        serde_json::json!({ "team": id, "user": user }),
    )
    .await;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /teams/{id}/leave`: the user leaves the team. The last member
/// leaving deletes it; the last owner of a team with other members must
/// appoint another owner first.
pub async fn leave(
    State(state): State<SharedState>,
    Auth(session): Auth,
    ip: ClientIp,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let user = session.user.id;
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    member_role(&tx, id, user).await?;
    lock_team(&tx, id).await?;
    let role = member_role(&tx, id, user).await?;
    let members: i64 = tx
        .query_one(
            "SELECT count(*) FROM team_members WHERE team_id = $1",
            &[&id],
        )
        .await?
        .get(0);
    let mut event = "team_left";
    if members <= 1 {
        delete_team(&tx, id).await?;
        event = "team_deleted";
    } else {
        if role == Role::Owner && owner_count(&tx, id).await? <= 1 {
            return Err(ApiError::conflict(
                "you are the only owner: make someone else an owner first, or delete the team",
            ));
        }
        tx.execute(
            "DELETE FROM team_members WHERE team_id = $1 AND user_id = $2",
            &[&id, &user],
        )
        .await?;
        drop_grants_of(&tx, id, user).await?;
    }
    audit(
        &tx,
        Some(user),
        event,
        ip.as_deref(),
        serde_json::json!({ "team": id }),
    )
    .await;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ----- invitations --------------------------------------------------------

const INVITE_COLUMNS: &str = "i.id, i.kind, i.expires_at, i.max_uses, i.uses, i.created_at,
    u.id, u.username, u.display_name, b.id, b.username, b.display_name";
const INVITE_JOINS: &str = "LEFT JOIN users u ON u.id = i.invitee_id
    LEFT JOIN users b ON b.id = i.invited_by";
/// An invitation that can still be used.
const INVITE_OPEN: &str = "i.status = 'pending'
    AND (i.expires_at IS NULL OR i.expires_at > now())
    AND (i.max_uses IS NULL OR i.uses < i.max_uses)";

fn invite_from_row(r: &tokio_postgres::Row) -> Invite {
    Invite {
        id: r.get(0),
        kind: r.get(1),
        expires_at: r.get(2),
        max_uses: r.get(3),
        uses: r.get(4),
        created_at: r.get(5),
        invitee: user_ref(r.get(6), r.get(7), r.get(8)),
        invited_by: user_ref(r.get(9), r.get(10), r.get(11)),
        url: None,
    }
}

/// Open invitations of a team (or just one of them).
async fn open_invites(
    db: &impl GenericClient,
    team: Uuid,
    only: Option<Uuid>,
) -> ApiResult<Vec<Invite>> {
    let rows = db
        .query(
            &format!(
                "SELECT {INVITE_COLUMNS} FROM team_invites i {INVITE_JOINS}
                 WHERE i.team_id = $1 AND ($2::uuid IS NULL OR i.id = $2) AND {INVITE_OPEN}
                 ORDER BY i.created_at"
            ),
            &[&team, &only],
        )
        .await?;
    Ok(rows.iter().map(invite_from_row).collect())
}

/// Options of an invite link.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkOptions {
    /// Days until the link expires (default 7, at most 365)
    pub expires_in_days: Option<i32>,
    /// How often it can be used; None = unlimited
    pub max_uses: Option<i32>,
}

/// `POST /teams/{id}/invites`: `{username}` or `{link: {…}}`.
#[derive(Debug, Deserialize)]
pub struct CreateInvite {
    /// Invite this user
    pub username: Option<String>,
    /// Create a link
    pub link: Option<LinkOptions>,
}

/// Invites a user, or creates an invite link (owners and admins).
pub async fn invite(
    State(state): State<SharedState>,
    Auth(session): Auth,
    ip: ClientIp,
    Path(id): Path<Uuid>,
    Json(req): Json<CreateInvite>,
) -> ApiResult<Json<Invite>> {
    let db = state.conn().await?;
    manager_role(&db, id, session.user.id).await?;
    let invite_id = Uuid::now_v7();
    let mut url = None;
    let (invite_id, details) = match (req.username, req.link) {
        (Some(username), None) => {
            let username = normalize_username(&username)
                .map_err(|_| ApiError::not_found("a user with this name"))?;
            let invitee: Uuid = db
                .query_opt(
                    "SELECT id FROM users WHERE username = $1 AND NOT disabled",
                    &[&username],
                )
                .await?
                .ok_or_else(|| ApiError::not_found("a user with this name"))?
                .get(0);
            if role_of(&db, id, invitee).await?.is_some() {
                return Err(ApiError::conflict(format!(
                    "{username} is already in the team"
                )));
            }
            db.execute(
                "INSERT INTO team_invites (id, team_id, invited_by, kind, invitee_id)
                 VALUES ($1, $2, $3, 'user', $4)
                 ON CONFLICT (team_id, invitee_id) WHERE kind = 'user' AND status = 'pending'
                 DO NOTHING",
                &[&invite_id, &id, &session.user.id, &invitee],
            )
            .await?;
            // inviting twice answers with the open invitation
            let existing: Uuid = db
                .query_one(
                    "SELECT id FROM team_invites
                     WHERE team_id = $1 AND invitee_id = $2 AND kind = 'user'
                       AND status = 'pending'",
                    &[&id, &invitee],
                )
                .await?
                .get(0);
            (existing, serde_json::json!({ "team": id, "user": invitee }))
        }
        (None, Some(link)) => {
            let days = link.expires_in_days.unwrap_or(LINK_DAYS.0);
            if !(1..=LINK_DAYS.1).contains(&days) {
                return Err(ApiError::bad_request("a link is valid for 1 to 365 days"));
            }
            if link.max_uses.is_some_and(|n| !(1..=10_000).contains(&n)) {
                return Err(ApiError::bad_request(
                    "a link can be used 1 to 10000 times, or without a limit",
                ));
            }
            let token = crypto::new_token();
            db.execute(
                "INSERT INTO team_invites (id, team_id, invited_by, kind, token_hash, max_uses,
                    expires_at)
                 VALUES ($1, $2, $3, 'link', $4, $5, now() + make_interval(days => $6))",
                &[
                    &invite_id,
                    &id,
                    &session.user.id,
                    &crypto::token_hash(&token),
                    &link.max_uses,
                    &days,
                ],
            )
            .await?;
            url = Some(format!(
                "{}/invite/{token}",
                state.config.public_url.trim_end_matches('/')
            ));
            (invite_id, serde_json::json!({ "team": id, "link": true }))
        }
        _ => {
            return Err(ApiError::bad_request("give a username or ask for a link"));
        }
    };
    audit(
        &db,
        Some(session.user.id),
        "team_invite_created",
        ip.as_deref(),
        details,
    )
    .await;
    let mut invite = open_invites(&db, id, Some(invite_id))
        .await?
        .pop()
        .ok_or_else(|| ApiError::internal("invite vanished"))?;
    invite.url = url;
    Ok(Json(invite))
}

/// `DELETE /teams/{id}/invites/{invite}`: withdraws an invitation (owners
/// and admins).
pub async fn revoke_invite(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Path((id, invite)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    let db = state.conn().await?;
    manager_role(&db, id, session.user.id).await?;
    let n = db
        .execute(
            "UPDATE team_invites SET status = 'revoked'
             WHERE id = $2 AND team_id = $1 AND status = 'pending'",
            &[&id, &invite],
        )
        .await?;
    if n == 0 {
        return Err(ApiError::not_found("invitation"));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// A team, as far as an invited user sees it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamRef {
    /// Id
    pub id: Uuid,
    /// Name
    pub name: String,
}

/// An invitation to the user.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MyInvite {
    /// Id
    pub id: Uuid,
    /// The team
    pub team: TeamRef,
    /// Who invited
    pub invited_by: Option<UserRef>,
    /// When
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// `GET /me/invites`: invitations waiting for the user's answer.
pub async fn my_invites(
    State(state): State<SharedState>,
    Auth(session): Auth,
) -> ApiResult<Json<Vec<MyInvite>>> {
    let rows = state
        .conn()
        .await?
        .query(
            &format!(
                "SELECT i.id, t.id, t.name, i.created_at, b.id, b.username, b.display_name
                 FROM team_invites i JOIN teams t ON t.id = i.team_id
                 LEFT JOIN users b ON b.id = i.invited_by
                 WHERE i.kind = 'user' AND i.invitee_id = $1 AND {INVITE_OPEN}
                 ORDER BY i.created_at"
            ),
            &[&session.user.id],
        )
        .await?;
    Ok(Json(
        rows.iter()
            .map(|r| MyInvite {
                id: r.get(0),
                team: TeamRef {
                    id: r.get(1),
                    name: r.get(2),
                },
                created_at: r.get(3),
                invited_by: user_ref(r.get(4), r.get(5), r.get(6)),
            })
            .collect(),
    ))
}

async fn join(db: &impl GenericClient, team: Uuid, user: Uuid, ip: &ClientIp, via: &str) -> bool {
    let joined = db
        .execute(
            "INSERT INTO team_members (team_id, user_id, role) VALUES ($1, $2, 'member')
             ON CONFLICT (team_id, user_id) DO NOTHING",
            &[&team, &user],
        )
        .await
        .is_ok_and(|n| n > 0);
    if joined {
        audit(
            db,
            Some(user),
            "team_joined",
            ip.as_deref(),
            serde_json::json!({ "team": team, "via": via }),
        )
        .await;
    }
    joined
}

async fn answer_invite(
    state: &SharedState,
    user: Uuid,
    invite: Uuid,
    ip: &ClientIp,
    accept: bool,
) -> ApiResult<Json<TeamRef>> {
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    let row = tx
        .query_opt(
            &format!(
                "SELECT t.id, t.name FROM team_invites i JOIN teams t ON t.id = i.team_id
                 WHERE i.id = $1 AND i.kind = 'user' AND i.invitee_id = $2 AND {INVITE_OPEN}
                 FOR UPDATE OF i"
            ),
            &[&invite, &user],
        )
        .await?
        .ok_or_else(|| ApiError::not_found("invitation"))?;
    let team = TeamRef {
        id: row.get(0),
        name: row.get(1),
    };
    let status = if accept { "accepted" } else { "declined" };
    tx.execute(
        "UPDATE team_invites SET status = $2 WHERE id = $1",
        &[&invite, &status],
    )
    .await?;
    if accept {
        join(&tx, team.id, user, ip, "invitation").await;
    }
    tx.commit().await?;
    Ok(Json(team))
}

/// `POST /invites/{id}/accept`: the user joins the team.
pub async fn accept_invite(
    State(state): State<SharedState>,
    Auth(session): Auth,
    ip: ClientIp,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<TeamRef>> {
    answer_invite(&state, session.user.id, id, &ip, true).await
}

/// `POST /invites/{id}/decline`
pub async fn decline_invite(
    State(state): State<SharedState>,
    Auth(session): Auth,
    ip: ClientIp,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<TeamRef>> {
    answer_invite(&state, session.user.id, id, &ip, false).await
}

/// `GET /invites/link/{token}`: what an invite link leads to.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkPreview {
    /// The team
    pub team: TeamRef,
    /// Who created the link
    pub invited_by: Option<UserRef>,
    /// The user is in the team already
    pub already_member: bool,
}

fn bad_link() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "this invite link is not valid (anymore)",
    )
}

/// Shows the team of an invite link.
pub async fn link_preview(
    State(state): State<SharedState>,
    Auth(session): Auth,
    Path(token): Path<String>,
) -> ApiResult<Json<LinkPreview>> {
    let db = state.conn().await?;
    let row = db
        .query_opt(
            &format!(
                "SELECT t.id, t.name, b.id, b.username, b.display_name
                 FROM team_invites i JOIN teams t ON t.id = i.team_id
                 LEFT JOIN users b ON b.id = i.invited_by
                 WHERE i.kind = 'link' AND i.token_hash = $1 AND {INVITE_OPEN}"
            ),
            &[&crypto::token_hash(&token)],
        )
        .await?
        .ok_or_else(bad_link)?;
    let team: Uuid = row.get(0);
    Ok(Json(LinkPreview {
        already_member: role_of(&db, team, session.user.id).await?.is_some(),
        team: TeamRef {
            id: team,
            name: row.get(1),
        },
        invited_by: user_ref(row.get(2), row.get(3), row.get(4)),
    }))
}

/// `POST /invites/link/{token}/accept`: joins the team of an invite link.
pub async fn accept_link(
    State(state): State<SharedState>,
    Auth(session): Auth,
    ip: ClientIp,
    Path(token): Path<String>,
) -> ApiResult<Json<TeamRef>> {
    let mut db = state.conn().await?;
    let tx = db.transaction().await?;
    let row = tx
        .query_opt(
            &format!(
                "SELECT i.id, t.id, t.name FROM team_invites i JOIN teams t ON t.id = i.team_id
                 WHERE i.kind = 'link' AND i.token_hash = $1 AND {INVITE_OPEN}
                 FOR UPDATE OF i"
            ),
            &[&crypto::token_hash(&token)],
        )
        .await?
        .ok_or_else(bad_link)?;
    let invite: Uuid = row.get(0);
    let team = TeamRef {
        id: row.get(1),
        name: row.get(2),
    };
    // members following the link again don't use it up
    if join(&tx, team.id, session.user.id, &ip, "link").await {
        tx.execute(
            "UPDATE team_invites SET uses = uses + 1 WHERE id = $1",
            &[&invite],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(team))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_names() {
        assert_eq!(team_name("  Sim \n Racers ").unwrap(), "Sim Racers");
        assert!(team_name("   ").is_err());
        assert!(team_name(&"x".repeat(65)).is_err());
        assert!(team_name(&"ä".repeat(64)).is_ok());
    }

    #[test]
    fn roles_are_ordered() {
        assert!(Role::Owner > Role::Admin && Role::Admin > Role::Member);
        for r in [Role::Owner, Role::Admin, Role::Member] {
            assert_eq!(Role::parse(r.as_str()).unwrap(), r);
        }
    }
}
