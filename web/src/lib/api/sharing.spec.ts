// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from 'vitest';
import { ServerApiClient, type Transport } from './sharing.ts';

function recorder() {
	const calls: { method: string; path: string; body?: unknown }[] = [];
	const send = ((method: string, path: string, body?: unknown) => {
		calls.push({ method, path, body });
		return Promise.resolve({});
	}) as Transport;
	return { calls, api: new ServerApiClient(send) };
}

describe('ServerApiClient', () => {
	it('sends only kind and id of the people and teams shared with', async () => {
		const { calls, api } = recorder();
		await api.setSessionSharing('s 1', {
			mode: 'custom',
			grants: [
				{ kind: 'user', id: 'u1', username: 'max', displayName: 'Max' },
				{ kind: 'team', id: 't1', name: 'Sim Racers' },
				{ kind: 'everyone' }
			]
		});
		expect(calls).toEqual([
			{
				method: 'PUT',
				path: '/sessions/s%201/sharing',
				body: {
					mode: 'custom',
					grants: [{ kind: 'user', id: 'u1' }, { kind: 'team', id: 't1' }, { kind: 'everyone' }]
				}
			}
		]);
	});

	it('builds team and invitation requests', async () => {
		const { calls, api } = recorder();
		await api.createInviteLink('t1', 7, null);
		await api.setMemberRole('t1', 'u1', 'admin');
		await api.lookupUser(' Max ');
		expect(calls).toEqual([
			{
				method: 'POST',
				path: '/teams/t1/invites',
				body: { link: { expiresInDays: 7, maxUses: null } }
			},
			{ method: 'PATCH', path: '/teams/t1/members/u1', body: { role: 'admin' } },
			{ method: 'GET', path: '/users/lookup?username=Max', body: undefined }
		]);
	});
});
