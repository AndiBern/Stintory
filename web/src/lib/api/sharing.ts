// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import type {
	DefaultSharing,
	Grantee,
	InviteLinkPreview,
	MyInvite,
	SharedSession,
	SharingState,
	SharingUpdate,
	TeamDetail,
	TeamInvite,
	TeamRef,
	TeamRole,
	TeamSummary,
	UserRef
} from './types.ts';

/**
 * Teams and sharing. They live on the server: the browser talks to it
 * directly, the desktop app through its backend (with the device token),
 * so it needs a connection to the server.
 */
export interface ServerApi {
	teams(): Promise<TeamSummary[]>;
	createTeam(name: string): Promise<TeamSummary>;
	team(id: string): Promise<TeamDetail>;
	renameTeam(id: string, name: string): Promise<TeamSummary>;
	deleteTeam(id: string): Promise<void>;
	setMemberRole(team: string, user: string, role: TeamRole): Promise<void>;
	removeMember(team: string, user: string): Promise<void>;
	leaveTeam(id: string): Promise<void>;
	inviteUser(team: string, username: string): Promise<TeamInvite>;
	/** The answer has the link (`url`); it is shown only once. */
	createInviteLink(
		team: string,
		expiresInDays: number,
		maxUses: number | null
	): Promise<TeamInvite>;
	revokeInvite(team: string, invite: string): Promise<void>;
	myInvites(): Promise<MyInvite[]>;
	acceptInvite(id: string): Promise<TeamRef>;
	declineInvite(id: string): Promise<TeamRef>;
	inviteLink(token: string): Promise<InviteLinkPreview>;
	acceptInviteLink(token: string): Promise<TeamRef>;

	/** Finds a user by exact username; rejects if there is none. */
	lookupUser(username: string): Promise<UserRef>;
	/** People from the user's teams matching `q`. */
	suggestUsers(q: string): Promise<UserRef[]>;

	defaultSharing(): Promise<DefaultSharing>;
	setDefaultSharing(grants: Grantee[]): Promise<DefaultSharing>;
	sessionSharing(id: string): Promise<SharingState>;
	setSessionSharing(id: string, update: SharingUpdate): Promise<SharingState>;
	lapSharing(id: string): Promise<SharingState>;
	setLapSharing(id: string, update: SharingUpdate): Promise<SharingState>;

	/** Sessions others shared with the user, newest first. */
	sharedSessions(): Promise<SharedSession[]>;
}

/** Sends one request of the server API (`path` relative to `/api/v1`). */
export type Transport = <T>(method: string, path: string, body?: unknown) => Promise<T>;

const enc = encodeURIComponent;

/** {@link ServerApi} on top of a way to send requests to the server. */
export class ServerApiClient implements ServerApi {
	constructor(private readonly send: Transport) {}

	teams(): Promise<TeamSummary[]> {
		return this.send('GET', '/teams');
	}

	createTeam(name: string): Promise<TeamSummary> {
		return this.send('POST', '/teams', { name });
	}

	team(id: string): Promise<TeamDetail> {
		return this.send('GET', `/teams/${enc(id)}`);
	}

	renameTeam(id: string, name: string): Promise<TeamSummary> {
		return this.send('PATCH', `/teams/${enc(id)}`, { name });
	}

	deleteTeam(id: string): Promise<void> {
		return this.send('DELETE', `/teams/${enc(id)}`);
	}

	setMemberRole(team: string, user: string, role: TeamRole): Promise<void> {
		return this.send('PATCH', `/teams/${enc(team)}/members/${enc(user)}`, { role });
	}

	removeMember(team: string, user: string): Promise<void> {
		return this.send('DELETE', `/teams/${enc(team)}/members/${enc(user)}`);
	}

	leaveTeam(id: string): Promise<void> {
		return this.send('POST', `/teams/${enc(id)}/leave`, {});
	}

	inviteUser(team: string, username: string): Promise<TeamInvite> {
		return this.send('POST', `/teams/${enc(team)}/invites`, { username });
	}

	createInviteLink(
		team: string,
		expiresInDays: number,
		maxUses: number | null
	): Promise<TeamInvite> {
		return this.send('POST', `/teams/${enc(team)}/invites`, {
			link: { expiresInDays, maxUses }
		});
	}

	revokeInvite(team: string, invite: string): Promise<void> {
		return this.send('DELETE', `/teams/${enc(team)}/invites/${enc(invite)}`);
	}

	myInvites(): Promise<MyInvite[]> {
		return this.send('GET', '/me/invites');
	}

	acceptInvite(id: string): Promise<TeamRef> {
		return this.send('POST', `/invites/${enc(id)}/accept`, {});
	}

	declineInvite(id: string): Promise<TeamRef> {
		return this.send('POST', `/invites/${enc(id)}/decline`, {});
	}

	inviteLink(token: string): Promise<InviteLinkPreview> {
		return this.send('GET', `/invites/link/${enc(token)}`);
	}

	acceptInviteLink(token: string): Promise<TeamRef> {
		return this.send('POST', `/invites/link/${enc(token)}/accept`, {});
	}

	lookupUser(username: string): Promise<UserRef> {
		return this.send('GET', `/users/lookup?username=${enc(username.trim())}`);
	}

	suggestUsers(q: string): Promise<UserRef[]> {
		return this.send('GET', `/users/suggest?q=${enc(q)}`);
	}

	defaultSharing(): Promise<DefaultSharing> {
		return this.send('GET', '/me/default-sharing');
	}

	setDefaultSharing(grants: Grantee[]): Promise<DefaultSharing> {
		return this.send('PUT', '/me/default-sharing', { grants: grants.map(grantRef) });
	}

	sessionSharing(id: string): Promise<SharingState> {
		return this.send('GET', `/sessions/${enc(id)}/sharing`);
	}

	setSessionSharing(id: string, update: SharingUpdate): Promise<SharingState> {
		return this.send('PUT', `/sessions/${enc(id)}/sharing`, updateBody(update));
	}

	lapSharing(id: string): Promise<SharingState> {
		return this.send('GET', `/laps/${enc(id)}/sharing`);
	}

	setLapSharing(id: string, update: SharingUpdate): Promise<SharingState> {
		return this.send('PUT', `/laps/${enc(id)}/sharing`, updateBody(update));
	}

	sharedSessions(): Promise<SharedSession[]> {
		return this.send('GET', '/shared/sessions');
	}
}

/** What the server needs of a grantee: its kind and id. */
function grantRef(g: Grantee): { kind: string; id?: string } {
	return g.kind === 'everyone' ? { kind: g.kind } : { kind: g.kind, id: g.id };
}

function updateBody(update: SharingUpdate) {
	return { mode: update.mode, grants: update.grants.map(grantRef) };
}
