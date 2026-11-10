// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { open } from '@tauri-apps/plugin-dialog';
import { ApiError, type ApiClient, type Unsubscribe } from './client.ts';
import type {
	AppInfo,
	CompareData,
	CompareRequest,
	ImportProgress,
	ImportResult,
	RecorderStatus,
	SessionDetail,
	SessionSummary,
	Settings
} from './types.ts';

/** ApiClient backed by the Rust commands of the desktop app (apps/desktop). */
export class TauriApi implements ApiClient {
	readonly capabilities = { importFiles: true, liveRecording: true, localSettings: true };

	getInfo(): Promise<AppInfo> {
		return call('get_info');
	}

	listSessions(): Promise<SessionSummary[]> {
		return call('list_sessions');
	}

	getSession(id: string): Promise<SessionDetail> {
		return call('get_session', { id });
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
			title: 'Import telemetry',
			filters: [{ name: 'iRacing telemetry', extensions: ['ibt'] }]
		});
		if (!picked || picked.length === 0) return null;
		return this.importFiles(picked);
	}

	importFiles(paths: string[]): Promise<ImportResult> {
		return call('import_files', { paths });
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

	onSessionsChanged(handler: () => void): Unsubscribe {
		return subscribe(listen('sessions-changed', () => handler()));
	}

	onImportProgress(handler: (p: ImportProgress) => void): Unsubscribe {
		return subscribe(listen<ImportProgress>('import-progress', (e) => handler(e.payload)));
	}

	onRecorderStatus(handler: (s: RecorderStatus) => void): Unsubscribe {
		return subscribe(listen<RecorderStatus>('recorder-status', (e) => handler(e.payload)));
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
