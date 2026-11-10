// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { api, type ImportProgress, type ImportResult, type RecorderStatus } from './api/index.ts';

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
	notice: null as Notice | null
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
		client.onFilesDropped((paths) => void runImport(() => client.importFiles(paths)))
	];
	client.recorderStatus().then((s) => {
		app.recorder = s;
	});
	return () => subs.forEach((unsubscribe) => unsubscribe());
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
		if (result) app.notice = { tone: 'ok', text: importSummary(result) };
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
