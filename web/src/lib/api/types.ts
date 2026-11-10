// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Data transfer types shared by both backends (Tauri desktop and rz-server).
// They mirror the Rust types in `rz-model` (crates/rz-model/src/api.rs).
// Keep both sides in sync until we generate these from Rust.

export type BackendKind = 'desktop' | 'server';

export type GameId = 'iracing' | 'f1_25' | 'lmu';

export type SessionType = 'practice' | 'qualifying' | 'race' | 'time_trial' | 'other';

export interface AppInfo {
	name: string;
	version: string;
	backend: BackendKind;
}

export interface SessionSummary {
	id: string;
	game: GameId;
	trackKey: string;
	trackName: string;
	carKey: string;
	carName: string;
	sessionType: SessionType;
	/** RFC 3339 timestamp */
	startedAt: string;
	lapCount: number;
	bestLapMs: number | null;
	origin: Origin;
}

/** Where a session came from. */
export type Origin =
	| { kind: 'own' }
	| { kind: 'imported'; driverName: string | null }
	| { kind: 'synced'; server: string; owner: string };

export interface LapSummary {
	id: string;
	number: number;
	lapTimeMs: number | null;
	sectorTimesMs: number[];
	valid: boolean;
	invalidReason: string | null;
	outLap: boolean;
	inLap: boolean;
	incomplete: boolean;
	flashback: boolean;
}

export interface ConditionsView {
	airTempC: number | null;
	trackTempC: number | null;
	weather: string | null;
}

export interface SessionDetail {
	summary: SessionSummary;
	endedAt: string | null;
	gameVersion: string | null;
	conditions: ConditionsView;
	laps: LapSummary[];
}

export interface CompareRequest {
	/** First lap is the reference for the delta */
	lapIds: string[];
	stepM?: number;
}

export interface LapTrace {
	lapId: string;
	sessionId: string;
	number: number;
	lapTimeMs: number | null;
	valid: boolean;
	/** channel name → values on the distance grid (null = no data) */
	channels: Record<string, (number | null)[]>;
	/** seconds vs. the reference lap, positive = slower */
	deltaS: (number | null)[];
}

export interface CompareData {
	stepM: number;
	distM: number[];
	units: Record<string, string>;
	laps: LapTrace[];
}

export interface ImportResult {
	files: number;
	sessions: number;
	laps: number;
	/** Laps of .rzx files that were already stored */
	skippedLaps: number;
}

/** What to export: whole sessions and/or single laps. */
export interface ExportSelection {
	sessionIds?: string[];
	lapIds?: string[];
}

export interface ExportRequest extends ExportSelection {
	/** Shown to whoever imports the file; null = anonymous */
	driverName: string | null;
	includeSetup: boolean;
}

export interface ExportResult {
	/** Written file, or directory for several CSV files */
	path: string;
	sessions: number;
	laps: number;
}

export interface ImportProgress {
	fraction: number;
	laps: number;
}

export interface Settings {
	dataDir: string;
	keepRawRecordings: boolean;
	rawStorageBytes: number;
	failedRecordings: number;
	/** Driver name of the last export */
	exportDriverName: string | null;
}

export type RecorderState = 'unsupported' | 'waiting' | 'recording';

export interface RecorderStatus {
	state: RecorderState;
	game: GameId | null;
	trackName: string | null;
	carName: string | null;
	laps: number;
	lastLapMs: number | null;
}

// ----- server accounts (HttpApi only) -----

/** The logged-in user. */
export interface Me {
	id: string;
	username: string;
	displayName: string;
	isAdmin: boolean;
	totpEnabled: boolean;
	hasPasskey: boolean;
	/** Must set a new password before anything else */
	mustChangePassword: boolean;
	/** Must set up a second factor first (enforced by the admin) */
	twoFactorRequired: boolean;
}

export interface AuthStatus {
	user: Me | null;
	csrfToken: string | null;
	registrationOpen: boolean;
}

export type LoginResponse =
	| { status: 'ok'; user: Me; csrfToken: string }
	| { status: 'secondFactor'; challenge: string; methods: string[] };

export interface TotpSetup {
	/** Base32, for manual entry */
	secret: string;
	uri: string;
	qrSvg: string;
}

export interface RecoveryCodes {
	recoveryCodes: string[];
}

/** A browser session of the user (account page). */
export interface BrowserSession {
	id: string;
	current: boolean;
	userAgent: string | null;
	ip: string | null;
	createdAt: string;
	lastSeenAt: string;
}

export interface AdminUser {
	id: string;
	username: string;
	displayName: string;
	isAdmin: boolean;
	disabled: boolean;
	totpEnabled: boolean;
	hasPasskey: boolean;
	mustChangePassword: boolean;
	createdAt: string;
}

/** A passkey of the logged-in user. */
export interface PasskeyInfo {
	id: string;
	name: string;
	createdAt: string;
	lastUsedAt: string | null;
}

export interface UserUpdate {
	disabled?: boolean;
	isAdmin?: boolean;
	resetPassword?: boolean;
	resetTwoFactor?: boolean;
}

export interface TemporaryPassword {
	id: string | null;
	temporaryPassword: string | null;
}

export interface ServerSettings {
	registration: 'closed' | 'open';
	enforce2fa: boolean;
	allowShareEveryone: boolean;
}

export interface AuditEntry {
	at: string;
	username: string | null;
	event: string;
	ip: string | null;
	details: Record<string, unknown>;
}

// ----- sync with a server (desktop) -----

export type SyncState = 'disconnected' | 'connecting' | 'idle' | 'syncing';

export interface SyncStatus {
	state: SyncState;
	serverUrl: string | null;
	username: string | null;
	deviceName: string | null;
	autoSync: boolean;
	/** false: the token is kept in the app's data folder (no keychain) */
	tokenInKeychain: boolean;
	lastSyncAt: string | null;
	lastPushed: number;
	lastPulled: number;
	lastError: string | null;
}

/** A desktop app connected to the user's account (server). */
export interface DeviceInfo {
	id: string;
	name: string;
	createdAt: string;
	lastUsedAt: string | null;
	lastIp: string | null;
}

export interface NewDevice {
	token: string;
	device: DeviceInfo;
	user: Me;
}
