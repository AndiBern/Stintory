// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import {
	accountApi,
	api,
	type ImportProgress,
	type ImportResult,
	type Me,
	type RecorderStatus,
	type SyncStatus
} from './api/index.ts';

/** A message shown at the top of the page until dismissed. */
export interface Notice {
	tone: 'ok' | 'error';
	text: string;
}

/** App-wide reactive state, fed by backend events (see `connect`). */
export const app = $state({
	/** Increments whenever sessions/laps changed; pages refetch on change. */
	sessionsVersion: 0,
	recorder: null as RecorderStatus | null,
	/** Running import, or null */
	importing: null as ImportProgress | null,
	/** Result of the last import or export */
	notice: null as Notice | null,
	/** Sync with a server (desktop app) */
	sync: null as SyncStatus | null
});

/** Subscribes to backend events. Returns a cleanup function. */
export function connect(): () => void {
	const client = api();
	const subs = [
		client.onSessionsChanged(() => {
			app.sessionsVersion++;
		}),
		client.onImportProgress((p) => {
			app.importing = p;
		}),
		client.onRecorderStatus((s) => {
			app.recorder = s;
		}),
		client.onSyncStatus((s) => {
			app.sync = s;
		}),
		client.onFilesDropped((paths) => void runImport(() => client.importFiles(paths)))
	];
	client.recorderStatus().then((s) => {
		app.recorder = s;
	});
	if (client.capabilities.serverSync) {
		client
			.syncStatus()
			.then((s) => {
				app.sync = s;
			})
			.catch(() => {});
	}
	return () => subs.forEach((unsubscribe) => unsubscribe());
}

/**
 * Teams and sharing can be used: logged in on the server, or the desktop
 * app is connected to one.
 */
export function sharingAvailable(): boolean {
	if (!accountApi()) return !!app.sync?.serverUrl;
	// restricted accounts (new password or 2FA needed first) can't use it yet
	return auth.me !== null && !auth.me.mustChangePassword && !auth.me.twoFactorRequired;
}

/** Text for a finished import. */
export function importSummary(r: ImportResult): string {
	const files = r.files === 1 ? '1 file' : `${r.files} files`;
	let text = `Imported ${files}: ${r.sessions} new session(s), ${r.laps} laps.`;
	if (r.skippedLaps > 0) text += ` ${r.skippedLaps} laps were already there.`;
	return text;
}

/** Runs an import and tracks its progress and result. */
export async function runImport(start: () => Promise<ImportResult | null>): Promise<void> {
	if (app.importing) return;
	app.importing = { fraction: 0, laps: 0 };
	app.notice = null;
	try {
		const result = await start();
		if (result) {
			app.notice = { tone: 'ok', text: importSummary(result) };
			app.sessionsVersion++; // the server sends no change events
		}
	} catch (e) {
		app.notice = { tone: 'error', text: `Import failed: ${errorText(e)}` };
	} finally {
		app.importing = null;
	}
}

/** Message of an error of any kind. */
export function errorText(e: unknown): string {
	return e instanceof Error ? e.message : String(e);
}

/** Login state when talking to sy-server (unused in the desktop app). */
export const auth = $state({
	/** The first status check finished */
	ready: false,
	/** The server answered (false: e.g. the static preview without server) */
	reachable: true,
	me: null as Me | null,
	registrationOpen: false
});

/** Asks the server who is logged in. */
export async function refreshAuth(): Promise<void> {
	const account = accountApi();
	if (!account) {
		auth.ready = true;
		return;
	}
	try {
		const s = await account.status();
		auth.me = s.user;
		auth.registrationOpen = s.registrationOpen;
		auth.reachable = true;
	} catch {
		auth.reachable = false;
	} finally {
		auth.ready = true;
	}
}

/** Watches for lost sessions. Returns a cleanup function. */
export function connectAuth(): () => void {
	const account = accountApi();
	if (!account) {
		auth.ready = true;
		return () => {};
	}
	account.onUnauthorized(() => {
		auth.me = null;
	});
	void refreshAuth();
	return () => account.onUnauthorized(() => {});
}
