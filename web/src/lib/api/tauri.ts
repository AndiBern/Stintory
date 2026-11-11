// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { open, save } from '@tauri-apps/plugin-dialog';
import { ApiError, type ApiClient, type Unsubscribe } from './client.ts';
import { ServerApiClient, type ServerApi } from './sharing.ts';
import type {
	AppInfo,
	CompareData,
	CompareRequest,
	ExportRequest,
	ExportResult,
	SyncStatus,
	ImportProgress,
	ImportResult,
	RecorderStatus,
	SessionDetail,
	SessionSummary,
	Settings
} from './types.ts';

/** ApiClient backed by the Rust commands of the desktop app (apps/desktop). */
export class TauriApi implements ApiClient {
	readonly capabilities = {
		importFiles: true,
		exportFiles: true,
		liveRecording: true,
		localSettings: true,
		serverSync: true
	};

	/** Passed on to the connected server by the backend (device token). */
	readonly server: ServerApi = new ServerApiClient((method, path, body) =>
		call('server_request', { method, path, body: body ?? null })
	);

	getInfo(): Promise<AppInfo> {
		return call('get_info');
	}

	listSessions(): Promise<SessionSummary[]> {
		return call('list_sessions');
	}

	async getSession(id: string, owner?: string): Promise<SessionDetail> {
		if (!owner) return call('get_session', { id });
		let local: string;
		try {
			// keeps (or refreshes) a local copy of the shared session
			local = await call<string>('open_shared_session', { owner, id });
		} catch (e) {
			// offline: the copy from last time, if there is one
			return call<SessionDetail>('get_session', { id }).catch(() => {
				throw e;
			});
		}
		return call('get_session', { id: local });
	}

	deleteSession(id: string): Promise<void> {
		return call('delete_session', { id });
	}

	compareLaps(request: CompareRequest): Promise<CompareData> {
		return call('compare_laps', { request });
	}

	async pickAndImportFiles(): Promise<ImportResult | null> {
		const picked = await open({
			multiple: true,
			directory: false,
			title: 'Import',
			filters: [
				{ name: 'Stintory export or iRacing telemetry', extensions: ['stintory', 'ibt'] },
				{ name: 'Stintory export', extensions: ['stintory'] },
				{ name: 'iRacing telemetry', extensions: ['ibt'] }
			]
		});
		if (!picked || picked.length === 0) return null;
		return this.importFiles(picked);
	}

	importFiles(paths: string[]): Promise<ImportResult> {
		return call('import_files', { paths });
	}

	async exportBundle(request: ExportRequest, suggestedName: string): Promise<ExportResult | null> {
		const path = await save({
			title: 'Export',
			defaultPath: `${suggestedName}.stintory`,
			filters: [{ name: 'Stintory export', extensions: ['stintory'] }]
		});
		if (!path) return null;
		return call('export_bundle', {
			request: { sessionIds: [], lapIds: [], ...request },
			path
		});
	}

	async exportCsv(lapIds: string[], suggestedName: string): Promise<ExportResult | null> {
		const path =
			lapIds.length === 1
				? await save({
						title: 'Export lap as CSV',
						defaultPath: `${suggestedName}.csv`,
						filters: [{ name: 'CSV', extensions: ['csv'] }]
					})
				: await open({ title: `Folder for ${lapIds.length} CSV files`, directory: true });
		if (!path) return null;
		return call('export_csv', { lapIds, path });
	}

	getSettings(): Promise<Settings> {
		return call('get_settings');
	}

	setKeepRawRecordings(keep: boolean): Promise<void> {
		return call('set_keep_raw_recordings', { keep });
	}

	deleteRawRecordings(): Promise<void> {
		return call('delete_raw_recordings');
	}

	recorderStatus(): Promise<RecorderStatus> {
		return call('recorder_status');
	}

	syncStatus(): Promise<SyncStatus> {
		return call('sync_status');
	}

	syncConnect(serverUrl: string): Promise<SyncStatus> {
		return call('sync_connect', { serverUrl });
	}

	syncCancelConnect(): Promise<void> {
		return call('sync_cancel_connect');
	}

	syncConnectToken(serverUrl: string, token: string): Promise<SyncStatus> {
		return call('sync_connect_token', { serverUrl, token });
	}

	syncDisconnect(): Promise<SyncStatus> {
		return call('sync_disconnect');
	}

	syncNow(): Promise<SyncStatus> {
		return call('sync_now');
	}

	syncSetAuto(auto: boolean): Promise<SyncStatus> {
		return call('sync_set_auto', { auto });
	}

	onSessionsChanged(handler: () => void): Unsubscribe {
		return subscribe(listen('sessions-changed', () => handler()));
	}

	onImportProgress(handler: (p: ImportProgress) => void): Unsubscribe {
		return subscribe(listen<ImportProgress>('import-progress', (e) => handler(e.payload)));
	}

	onRecorderStatus(handler: (s: RecorderStatus) => void): Unsubscribe {
		return subscribe(listen<RecorderStatus>('recorder-status', (e) => handler(e.payload)));
	}

	onSyncStatus(handler: (s: SyncStatus) => void): Unsubscribe {
		return subscribe(listen<SyncStatus>('sync-status', (e) => handler(e.payload)));
	}

	onFilesDropped(handler: (paths: string[]) => void): Unsubscribe {
		return subscribe(
			getCurrentWebview().onDragDropEvent((e) => {
				if (e.payload.type === 'drop' && e.payload.paths.length > 0) handler(e.payload.paths);
			})
		);
	}
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
	try {
		return await invoke<T>(command, args);
	} catch (e) {
		throw new ApiError(typeof e === 'string' ? e : String(e));
	}
}

/** Turns Tauri's async `listen` into a synchronous unsubscribe function. */
function subscribe(pending: Promise<UnlistenFn>): Unsubscribe {
	let unlisten: UnlistenFn | undefined;
	let cancelled = false;
	pending.then((fn) => {
		if (cancelled) fn();
		else unlisten = fn;
	});
	return () => {
		cancelled = true;
		unlisten?.();
	};
}
