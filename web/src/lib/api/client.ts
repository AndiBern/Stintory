// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import type {
	AppInfo,
	CompareData,
	CompareRequest,
	ExportRequest,
	ExportResult,
	F1CaptureSettings,
	LmuCaptureSettings,
	ProtonBridgeSettings,
	SyncStatus,
	ImportProgress,
	ImportResult,
	RecorderStatus,
	ReferenceLap,
	ReferenceScope,
	SessionDetail,
	SessionSummary,
	Settings
} from './types.ts';
import type { ServerApi } from './sharing.ts';

/** Stops listening to an event. */
export type Unsubscribe = () => void;

/**
 * The one API shape the frontend talks to.
 *
 * - In the desktop app it is implemented by {@link TauriApi} (Tauri `invoke`).
 * - In the browser it is implemented by {@link HttpApi} (sy-server REST API).
 *
 * Views must only use this interface, never a concrete implementation.
 * Features that only exist on the desktop (file import, live recording,
 * local settings) are announced in {@link ApiClient.capabilities}.
 */
export interface ApiClient {
	readonly capabilities: {
		/** Import telemetry files from the local disk */
		importFiles: boolean;
		/** Export to files on the local disk (.stintory, CSV) */
		exportFiles: boolean;
		/** Live recording while driving */
		liveRecording: boolean;
		/** Local client settings */
		localSettings: boolean;
		/** Sync with a Stintory server (desktop app) */
		serverSync: boolean;
	};

	/**
	 * Teams and sharing on the server. In the desktop app the calls need a
	 * connection to a server (see `sharingAvailable` in the app state).
	 */
	readonly server: ServerApi;

	getInfo(): Promise<AppInfo>;
	listSessions(): Promise<SessionSummary[]>;
	/**
	 * A session with its laps. With `owner` (a user id): a session that user
	 * shared, with the laps the user may see. The desktop app keeps a copy
	 * of it as a reference; the answer's id is the one to use from then on.
	 */
	getSession(id: string, owner?: string): Promise<SessionDetail>;
	deleteSession(id: string): Promise<void>;
	compareLaps(request: CompareRequest): Promise<CompareData>;
	/**
	 * Laps to compare with for a track and car, fastest first: the user's own
	 * (recorded and imported) and those others shared with them.
	 */
	referenceLaps(trackKey: string, carKey: string, scope?: ReferenceScope): Promise<ReferenceLap[]>;
	/**
	 * Makes chosen reference laps ready for {@link compareLaps}. The desktop app
	 * keeps a local copy of shared laps that are only on the server so far.
	 */
	prepareLaps(laps: ReferenceLap[]): Promise<void>;

	/** Opens a file picker and imports the chosen files; null if cancelled. */
	pickAndImportFiles(): Promise<ImportResult | null>;
	importFiles(paths: string[]): Promise<ImportResult>;
	/**
	 * Asks where to save, then writes an .stintory file; null if cancelled.
	 * `suggestedName` is the file name proposed in the dialog.
	 */
	exportBundle(request: ExportRequest, suggestedName: string): Promise<ExportResult | null>;
	/**
	 * Writes lap telemetry as CSV: asks for a file (one lap) or a folder
	 * (several laps); null if cancelled.
	 */
	exportCsv(lapIds: string[], suggestedName: string): Promise<ExportResult | null>;
	getSettings(): Promise<Settings>;
	setKeepRawRecordings(keep: boolean): Promise<void>;
	/** Applied as soon as no F1 session is being recorded. */
	setF1Capture(settings: F1CaptureSettings): Promise<void>;
	setLmuCapture(settings: LmuCaptureSettings): Promise<void>;
	/** Linux: applied as soon as no game is being recorded through the bridge. */
	setProtonBridge(settings: ProtonBridgeSettings): Promise<void>;
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
