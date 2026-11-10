// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { ApiError, notAvailable, type ApiClient, type Unsubscribe } from './client.ts';
import type {
	AppInfo,
	CompareData,
	CompareRequest,
	ImportResult,
	RecorderStatus,
	SessionDetail,
	SessionSummary,
	Settings
} from './types.ts';

const noop: Unsubscribe = () => {};

/**
 * ApiClient backed by the rz-server REST API (`/api/v1`). The server comes
 * with milestone M3; desktop-only features reject with a clear error.
 */
export class HttpApi implements ApiClient {
	readonly capabilities = { importFiles: false, liveRecording: false, localSettings: false };

	constructor(
		private readonly baseUrl = '/api/v1',
		private readonly fetchFn: typeof fetch = fetch.bind(globalThis)
	) {}

	getInfo(): Promise<AppInfo> {
		return this.request<AppInfo>('GET', '/info');
	}

	listSessions(): Promise<SessionSummary[]> {
		return this.request<SessionSummary[]>('GET', '/sessions');
	}

	getSession(id: string): Promise<SessionDetail> {
		return this.request<SessionDetail>('GET', `/sessions/${encodeURIComponent(id)}`);
	}

	deleteSession(id: string): Promise<void> {
		return this.request<void>('DELETE', `/sessions/${encodeURIComponent(id)}`);
	}

	compareLaps(request: CompareRequest): Promise<CompareData> {
		return this.request<CompareData>('POST', '/analysis/compare', request);
	}

	pickAndImportFiles(): Promise<ImportResult | null> {
		return Promise.reject(notAvailable('Importing files'));
	}

	importFiles(): Promise<ImportResult> {
		return Promise.reject(notAvailable('Importing files'));
	}

	getSettings(): Promise<Settings> {
		return Promise.reject(notAvailable('Local settings'));
	}

	setKeepRawRecordings(): Promise<void> {
		return Promise.reject(notAvailable('Local settings'));
	}

	deleteRawRecordings(): Promise<void> {
		return Promise.reject(notAvailable('Local settings'));
	}

	recorderStatus(): Promise<RecorderStatus> {
		return Promise.resolve({
			state: 'unsupported',
			game: null,
			trackName: null,
			carName: null,
			laps: 0,
			lastLapMs: null
		});
	}

	onSessionsChanged(): Unsubscribe {
		return noop;
	}

	onImportProgress(): Unsubscribe {
		return noop;
	}

	onRecorderStatus(): Unsubscribe {
		return noop;
	}

	onFilesDropped(): Unsubscribe {
		return noop;
	}

	private async request<T>(method: string, path: string, body?: unknown): Promise<T> {
		let response: Response;
		try {
			response = await this.fetchFn(this.baseUrl + path, {
				method,
				headers: {
					Accept: 'application/json',
					...(body !== undefined ? { 'Content-Type': 'application/json' } : {})
				},
				body: body !== undefined ? JSON.stringify(body) : undefined,
				credentials: 'same-origin'
			});
		} catch (e) {
			throw new ApiError(`Server not reachable: ${String(e)}`);
		}
		if (!response.ok) {
			throw new ApiError(`${response.status} ${response.statusText}`, response.status);
		}
		if (response.status === 204) return undefined as T;
		return (await response.json()) as T;
	}
}
