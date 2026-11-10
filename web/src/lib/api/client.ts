// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

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

/** Stops listening to an event. */
export type Unsubscribe = () => void;

/**
 * The one API shape the frontend talks to.
 *
 * - In the desktop app it is implemented by {@link TauriApi} (Tauri `invoke`).
 * - In the browser it is implemented by {@link HttpApi} (rz-server REST API).
 *
 * Views must only use this interface, never a concrete implementation.
 * Features that only exist on the desktop (file import, live recording,
 * local settings) are announced in {@link ApiClient.capabilities}.
 */
export interface ApiClient {
	readonly capabilities: {
		/** Import telemetry files from the local disk */
		importFiles: boolean;
		/** Export to files on the local disk (.rzx, CSV) */
		exportFiles: boolean;
		/** Live recording while driving */
		liveRecording: boolean;
		/** Local client settings */
		localSettings: boolean;
		/** Sync with a Rundenzeit server (desktop app) */
		serverSync: boolean;
	};

	getInfo(): Promise<AppInfo>;
	listSessions(): Promise<SessionSummary[]>;
	getSession(id: string): Promise<SessionDetail>;
	deleteSession(id: string): Promise<void>;
	compareLaps(request: CompareRequest): Promise<CompareData>;

	/** Opens a file picker and imports the chosen files; null if cancelled. */
	pickAndImportFiles(): Promise<ImportResult | null>;
	importFiles(paths: string[]): Promise<ImportResult>;
	/**
	 * Asks where to save, then writes an .rzx file; null if cancelled.
	 * `suggestedName` is the file name proposed in the dialog.
	 */
	exportRzx(request: ExportRequest, suggestedName: string): Promise<ExportResult | null>;
	/**
	 * Writes lap telemetry as CSV: asks for a file (one lap) or a folder
	 * (several laps); null if cancelled.
	 */
	exportCsv(lapIds: string[], suggestedName: string): Promise<ExportResult | null>;
	getSettings(): Promise<Settings>;
	setKeepRawRecordings(keep: boolean): Promise<void>;
	deleteRawRecordings(): Promise<void>;
	recorderStatus(): Promise<RecorderStatus>;

	syncStatus(): Promise<SyncStatus>;
	/** Opens the server's login in the browser; resolves once connected. */
	syncConnect(serverUrl: string): Promise<SyncStatus>;
	syncCancelConnect(): Promise<void>;
	syncConnectToken(serverUrl: string, token: string): Promise<SyncStatus>;
	syncDisconnect(): Promise<SyncStatus>;
	syncNow(): Promise<SyncStatus>;
	syncSetAuto(auto: boolean): Promise<SyncStatus>;

	onSessionsChanged(handler: () => void): Unsubscribe;
	onImportProgress(handler: (p: ImportProgress) => void): Unsubscribe;
	onRecorderStatus(handler: (s: RecorderStatus) => void): Unsubscribe;
	onSyncStatus(handler: (s: SyncStatus) => void): Unsubscribe;
	/** Files dropped onto the window (desktop only). */
	onFilesDropped(handler: (paths: string[]) => void): Unsubscribe;
}

/** Error raised by any ApiClient implementation. */
export class ApiError extends Error {
	constructor(
		message: string,
		readonly status?: number,
		/** Machine-readable reason from the server, e.g. `wrong_code` */
		readonly code?: string
	) {
		super(message);
		this.name = 'ApiError';
	}
}

/** Error for features the current backend doesn't have. */
export function notAvailable(feature: string): ApiError {
	return new ApiError(`${feature} is only available in the desktop app`);
}
