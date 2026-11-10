// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { api, type ImportProgress, type ImportResult, type RecorderStatus } from './api/index.ts';

/** App-wide reactive state, fed by backend events (see `connect`). */
export const app = $state({
	/** Increments whenever sessions/laps changed; pages refetch on change. */
	sessionsVersion: 0,
	recorder: null as RecorderStatus | null,
	/** Running import, or null */
	importing: null as ImportProgress | null,
	/** Result or error of the last import, shown as a notice */
	lastImport: null as { result?: ImportResult; error?: string } | null
});

/** Subscribes to backend events. Returns a cleanup function. */
export function connect(): () => void {
	const client = api();
	const subs = [
		client.onSessionsChanged(() => app.sessionsVersion++),
		client.onImportProgress((p) => (app.importing = p)),
		client.onRecorderStatus((s) => (app.recorder = s)),
		client.onFilesDropped((paths) => void runImport(() => client.importFiles(paths)))
	];
	client.recorderStatus().then((s) => (app.recorder = s));
	return () => subs.forEach((unsubscribe) => unsubscribe());
}

/** Runs an import and tracks its progress and result. */
export async function runImport(start: () => Promise<ImportResult | null>): Promise<void> {
	if (app.importing) return;
	app.importing = { fraction: 0, laps: 0 };
	app.lastImport = null;
	try {
		const result = await start();
		if (result) app.lastImport = { result };
	} catch (e) {
		app.lastImport = { error: e instanceof Error ? e.message : String(e) };
	} finally {
		app.importing = null;
	}
}
