// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Data transfer types shared by both backends (Tauri desktop and sy-server).
// They mirror the Rust types in `sy-model` (crates/sy-model/src/api.rs).
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
	/** Others can see the session (server only) */
	shared?: boolean;
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
	/**
	 * Who drove the lap, if not the user: the owner of a lap someone shared,
	 * the driver named in an imported file
	 */
	driver?: string | null;
	/** The owner of a lap someone shared (server only) */
	owner?: UserRef | null;
}

/** Which laps a reference list contains. */
export type ReferenceScope = 'all' | 'mine' | 'shared';

/**
 * A lap to compare with: one of the user's own (recorded or imported) or one
 * someone shared, for a track and car. Lists are sorted fastest first.
 */
export interface ReferenceLap {
	lapId: string;
	sessionId: string;
	number: number;
	lapTimeMs: number;
	sectorTimesMs: number[];
	game: GameId;
	trackKey: string;
	trackName: string;
	carKey: string;
	carName: string;
	sessionType: SessionType;
	sessionStartedAt: string;
	origin: Origin;
	/** The owner, for laps someone shared on the server; null for own laps */
	owner: UserRef | null;
	/** Desktop app: only on the server so far ({@link ApiClient.prepareLaps} keeps a copy) */
	remote: boolean;
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
	/** Laps of .stintory files that were already stored */
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
	/** Recording F1 25 (UDP telemetry) */
	f1Capture: F1CaptureSettings;
	/** Where the app listens for F1 packets right now, e.g. `127.0.0.1:20777` */
	f1Listening: string | null;
	/** Why the app can't listen for F1 packets */
	f1Error: string | null;
}

/** Settings for recording F1 25 (UDP telemetry). */
export interface F1CaptureSettings {
	enabled: boolean;
	/** UDP port; the game's default is 20777 */
	port: number;
	/** Also accept packets from consoles and other PCs */
	allowRemote: boolean;
	/** Forward every packet to these `host:port` addresses (other apps) */
	forward: string[];
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

// ----- teams and sharing (on the server) -----

/** Another user, as far as others may see them. */
export interface UserRef {
	id: string;
	username: string;
	displayName: string;
}

export type TeamRole = 'owner' | 'admin' | 'member';

export interface TeamSummary {
	id: string;
	name: string;
	/** The user's own role */
	role: TeamRole;
	memberCount: number;
	createdAt: string;
}

export interface TeamMember {
	user: UserRef;
	role: TeamRole;
	joinedAt: string;
}

export interface TeamInvite {
	id: string;
	kind: 'user' | 'link';
	invitee: UserRef | null;
	invitedBy: UserRef | null;
	expiresAt: string | null;
	/** null = unlimited */
	maxUses: number | null;
	uses: number;
	createdAt: string;
	/** The link; only in the answer that creates it */
	url?: string;
}

export interface TeamDetail extends TeamSummary {
	members: TeamMember[];
	/** Open invitations (owners and admins only) */
	invites: TeamInvite[];
}

export interface TeamRef {
	id: string;
	name: string;
}

/** An invitation waiting for the user's answer. */
export interface MyInvite {
	id: string;
	team: TeamRef;
	invitedBy: UserRef | null;
	createdAt: string;
}

export interface InviteLinkPreview {
	team: TeamRef;
	invitedBy: UserRef | null;
	alreadyMember: boolean;
}

/** Who something is shared with. */
export type Grantee =
	| { kind: 'user'; id: string; username: string; displayName: string }
	| { kind: 'team'; id: string; name: string }
	| { kind: 'everyone' };

export type SharingMode = 'inherit' | 'custom';

/** Where the effective audience comes from. */
export type SharingSource = 'own' | 'session' | 'default' | 'imported';

export interface SharingState {
	mode: SharingMode;
	/** Own grants (empty with `inherit`) */
	grants: Grantee[];
	source: SharingSource;
	/** Who can see it right now */
	effective: Grantee[];
	/** The admin allows sharing with everyone */
	everyoneAllowed: boolean;
}

export interface SharingUpdate {
	mode: SharingMode;
	grants: Grantee[];
}

export interface DefaultSharing {
	grants: Grantee[];
	everyoneAllowed: boolean;
	/** Sessions that use the default */
	inheritingSessions: number;
}

/** A session someone shared with the user. */
export interface SharedSession extends SessionSummary {
	owner: UserRef;
}

/** A track and car with laps others shared with the user. */
export interface SharedTrack {
	game: GameId;
	trackKey: string;
	trackName: string;
	carKey: string;
	carName: string;
	/** Shared laps that count for a best lap */
	laps: number;
	/** People who shared them */
	drivers: number;
	bestLapMs: number | null;
	bestDriver: UserRef | null;
	/** The user's own fastest lap here */
	ownBestLapMs: number | null;
	/** Start of the newest session with a shared lap */
	latestAt: string;
}
