// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { ApiError, type ApiClient } from './client.ts';
import type { AppInfo, SessionSummary } from './types.ts';

/** ApiClient backed by the rz-server REST API (`/api/v1`). */
export class HttpApi implements ApiClient {
	constructor(
		private readonly baseUrl = '/api/v1',
		private readonly fetchFn: typeof fetch = fetch.bind(globalThis)
	) {}

	getInfo(): Promise<AppInfo> {
		return this.get<AppInfo>('/info');
	}

	listSessions(): Promise<SessionSummary[]> {
		return this.get<SessionSummary[]>('/sessions');
	}

	private async get<T>(path: string): Promise<T> {
		let response: Response;
		try {
			response = await this.fetchFn(this.baseUrl + path, {
				headers: { Accept: 'application/json' },
				credentials: 'same-origin'
			});
		} catch (e) {
			throw new ApiError(`Server not reachable: ${String(e)}`);
		}
		if (!response.ok) {
			throw new ApiError(`${response.status} ${response.statusText}`, response.status);
		}
		return (await response.json()) as T;
	}
}
