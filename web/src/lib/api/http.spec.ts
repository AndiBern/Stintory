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
});
