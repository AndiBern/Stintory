// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from 'vitest';
import { ApiError } from './client.ts';
import { HttpApi } from './http.ts';

function fakeFetch(status: number, body: unknown): typeof fetch {
	return async () =>
		new Response(JSON.stringify(body), {
			status,
			headers: { 'Content-Type': 'application/json' }
		});
}

describe('HttpApi', () => {
	it('parses JSON responses', async () => {
		const info = { name: 'Rundenzeit', version: '0.1.0', backend: 'server' };
		const api = new HttpApi('/api/v1', fakeFetch(200, info));
		await expect(api.getInfo()).resolves.toEqual(info);
	});

	it('turns HTTP errors into ApiError with status', async () => {
		const api = new HttpApi('/api/v1', fakeFetch(503, {}));
		const error = await api.listSessions().catch((e: unknown) => e);
		expect(error).toBeInstanceOf(ApiError);
		expect((error as ApiError).status).toBe(503);
	});

	it('turns network failures into ApiError', async () => {
		const api = new HttpApi('/api/v1', async () => {
			throw new TypeError('connection refused');
		});
		await expect(api.getInfo()).rejects.toBeInstanceOf(ApiError);
	});

	it('sends the CSRF token on changing requests only', async () => {
		const seen: Array<[string, string | null]> = [];
		const api = new HttpApi('/api/v1', async (_url, init) => {
			const headers = new Headers(init?.headers);
			seen.push([init?.method ?? 'GET', headers.get('X-CSRF-Token')]);
			return new Response(
				JSON.stringify({ user: null, csrfToken: 'tok', registrationOpen: false }),
				{ status: 200, headers: { 'Content-Type': 'application/json' } }
			);
		});
		await api.status();
		await api.listSessions();
		await api.updateMe('x');
		expect(seen).toEqual([
			['GET', null],
			['GET', null],
			['PATCH', 'tok']
		]);
	});

	it('reads error codes and reports lost sessions', async () => {
		let lost = 0;
		const api = new HttpApi('/api/v1', fakeFetch(403, { error: 'nope', code: 'csrf' }));
		api.onUnauthorized(() => lost++);
		const error = (await api.listSessions().catch((e: unknown) => e)) as ApiError;
		expect([error.message, error.code, error.status]).toEqual(['nope', 'csrf', 403]);

		const gone = new HttpApi('/api/v1', fakeFetch(401, { error: 'not logged in' }));
		gone.onUnauthorized(() => lost++);
		await gone.listSessions().catch(() => {});
		// a wrong password is no lost session
		await gone.login('a', 'b').catch(() => {});
		expect(lost).toBe(1);
	});
});
