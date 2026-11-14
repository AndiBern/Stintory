// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { ApiError, notAvailable, type ApiClient, type Unsubscribe } from './client.ts';
import { createPasskey, getPasskey, type JsonOptions } from '../webauthn.ts';
import { ServerApiClient, type ServerApi } from './sharing.ts';
import type {
	AdminUser,
	AppInfo,
	AuditEntry,
	AuthStatus,
	BrowserSession,
	CompareData,
	CompareRequest,
	ExportResult,
	ImportResult,
	LoginResponse,
	Me,
	RecorderStatus,
	RecoveryCodes,
	ReferenceLap,
	ReferenceScope,
	ServerSettings,
	SessionDetail,
	SessionSummary,
	DeviceInfo,
	NewDevice,
	OAuthAuthorizeInfo,
	TokenScope,
	PasskeyInfo,
	Settings,
	SyncStatus,
	TemporaryPassword,
	TotpSetup,
	UserUpdate
} from './types.ts';

const noop: Unsubscribe = () => {};

/** Account features of the server (login, 2FA, administration). */
export interface AccountApi {
	status(): Promise<AuthStatus>;
	login(username: string, password: string): Promise<LoginResponse>;
	loginSecondFactor(challenge: string, code: string): Promise<LoginResponse>;
	register(username: string, password: string, displayName: string): Promise<LoginResponse>;
	logout(): Promise<void>;
	/** Passwordless login: the browser asks for one of the user's passkeys. */
	loginWithPasskey(): Promise<LoginResponse>;
	/** Second step after the password, with a passkey. */
	loginSecondFactorPasskey(challenge: string): Promise<LoginResponse>;
	passkeys(): Promise<PasskeyInfo[]>;
	/** Creates a passkey in the browser and registers it. */
	addPasskey(password: string, name: string): Promise<PasskeyInfo>;
	removePasskey(id: string): Promise<void>;
	me(): Promise<Me>;
	updateMe(displayName: string): Promise<void>;
	/** `disconnectDevices`: also disconnect all desktop apps and AI assistants. */
	changePassword(
		currentPassword: string,
		newPassword: string,
		disconnectDevices: boolean
	): Promise<void>;
	totpSetup(): Promise<TotpSetup>;
	totpConfirm(code: string): Promise<RecoveryCodes>;
	totpDisable(password: string): Promise<void>;
	regenerateRecoveryCodes(password: string): Promise<RecoveryCodes>;
	browserSessions(): Promise<BrowserSession[]>;
	endBrowserSession(id: string): Promise<void>;
	devices(): Promise<DeviceInfo[]>;
	createDevice(name: string, scope?: TokenScope): Promise<NewDevice>;
	revokeDevice(id: string): Promise<void>;
	/** Approves a desktop app's login; returns where to send the browser. */
	approveDeviceLogin(request: {
		redirectUri: string;
		codeChallenge: string;
		state: string;
		deviceName: string;
	}): Promise<{ redirectTo: string }>;
	/** Checks an OAuth authorization request (`query`: the page's search string). */
	oauthAuthorizeInfo(query: string): Promise<OAuthAuthorizeInfo>;
	/** Allows or denies an OAuth client; returns where the browser goes next. */
	oauthApprove(params: Record<string, string>, allow: boolean): Promise<{ redirectTo: string }>;
	adminUsers(): Promise<AdminUser[]>;
	adminCreateUser(
		username: string,
		displayName: string,
		isAdmin: boolean
	): Promise<TemporaryPassword>;
	adminUpdateUser(id: string, update: UserUpdate): Promise<TemporaryPassword>;
	adminSettings(): Promise<ServerSettings>;
	adminSaveSettings(settings: ServerSettings): Promise<ServerSettings>;
	adminAudit(limit?: number): Promise<AuditEntry[]>;
	/** Called when a request finds the session gone (401). */
	onUnauthorized(handler: () => void): void;
}

/**
 * ApiClient backed by the sy-server REST API (`/api/v1`), plus the account
 * features of the server ({@link AccountApi}).
 *
 * Changing requests carry the session's CSRF token (`X-CSRF-Token`), which
 * the server hands out with the login and with `GET /auth/status`.
 */
export class HttpApi implements ApiClient, AccountApi {
	readonly capabilities = {
		importFiles: true,
		exportFiles: false,
		liveRecording: false,
		localSettings: false,
		serverSync: false
	};

	private csrfToken: string | null = null;
	private unauthorized: (() => void) | null = null;

	readonly server: ServerApi = new ServerApiClient((method, path, body) =>
		this.request(method, path, body)
	);

	constructor(
		private readonly baseUrl = '/api/v1',
		private readonly fetchFn: typeof fetch = fetch.bind(globalThis)
	) {}

	// ----- ApiClient -----

	getInfo(): Promise<AppInfo> {
		return this.request<AppInfo>('GET', '/info');
	}

	listSessions(): Promise<SessionSummary[]> {
		return this.request<SessionSummary[]>('GET', '/sessions');
	}

	getSession(id: string, owner?: string): Promise<SessionDetail> {
		const path = owner
			? `/shared/sessions/${encodeURIComponent(owner)}/${encodeURIComponent(id)}`
			: `/sessions/${encodeURIComponent(id)}`;
		return this.request<SessionDetail>('GET', path);
	}

	deleteSession(id: string): Promise<void> {
		return this.request<void>('DELETE', `/sessions/${encodeURIComponent(id)}`);
	}

	compareLaps(request: CompareRequest): Promise<CompareData> {
		return this.request<CompareData>('POST', '/analysis/compare', request);
	}

	referenceLaps(
		trackKey: string,
		carKey: string,
		scope: ReferenceScope = 'all'
	): Promise<ReferenceLap[]> {
		const q = new URLSearchParams({ track: trackKey, car: carKey, scope });
		return this.request<ReferenceLap[]>('GET', `/reference-laps?${q}`);
	}

	/** Nothing to do: the server compares any lap the user may read. */
	prepareLaps(): Promise<void> {
		return Promise.resolve();
	}

	/** Lets the user pick .stintory files and uploads them one by one. */
	async pickAndImportFiles(): Promise<ImportResult | null> {
		const files = await pickFiles('.stintory');
		if (files.length === 0) return null;
		const total: ImportResult = { files: 0, sessions: 0, laps: 0, skippedLaps: 0 };
		for (const file of files) {
			let r: ImportResult;
			try {
				r = await this.request<ImportResult>('POST', '/import', file);
			} catch (e) {
				throw new ApiError(`${file.name}: ${e instanceof Error ? e.message : e}`);
			}
			total.files += r.files;
			total.sessions += r.sessions;
			total.laps += r.laps;
			total.skippedLaps += r.skippedLaps;
		}
		return total;
	}

	importFiles(): Promise<ImportResult> {
		return Promise.reject(notAvailable('Importing files from disk paths'));
	}

	exportBundle(): Promise<ExportResult | null> {
		return Promise.reject(notAvailable('Exporting files'));
	}

	exportCsv(): Promise<ExportResult | null> {
		return Promise.reject(notAvailable('Exporting files'));
	}

	getSettings(): Promise<Settings> {
		return Promise.reject(notAvailable('Local settings'));
	}

	setKeepRawRecordings(): Promise<void> {
		return Promise.reject(notAvailable('Local settings'));
	}

	setF1Capture(): Promise<void> {
		return Promise.reject(notAvailable('Recording'));
	}

	setLmuCapture(): Promise<void> {
		return Promise.reject(notAvailable('Recording'));
	}

	setProtonBridge(): Promise<void> {
		return Promise.reject(notAvailable('Recording'));
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

	syncStatus(): Promise<SyncStatus> {
		return Promise.reject(notAvailable('Sync'));
	}

	syncConnect(): Promise<SyncStatus> {
		return Promise.reject(notAvailable('Sync'));
	}

	syncCancelConnect(): Promise<void> {
		return Promise.resolve();
	}

	syncConnectToken(): Promise<SyncStatus> {
		return Promise.reject(notAvailable('Sync'));
	}

	syncDisconnect(): Promise<SyncStatus> {
		return Promise.reject(notAvailable('Sync'));
	}

	syncNow(): Promise<SyncStatus> {
		return Promise.reject(notAvailable('Sync'));
	}

	syncSetAuto(): Promise<SyncStatus> {
		return Promise.reject(notAvailable('Sync'));
	}

	onSessionsChanged(): Unsubscribe {
		return noop;
	}

	onSyncStatus(): Unsubscribe {
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

	// ----- AccountApi -----

	async status(): Promise<AuthStatus> {
		const s = await this.request<AuthStatus>('GET', '/auth/status');
		this.csrfToken = s.csrfToken;
		return s;
	}

	async login(username: string, password: string): Promise<LoginResponse> {
		return this.loggedIn(
			await this.request<LoginResponse>('POST', '/auth/login', { username, password })
		);
	}

	async loginSecondFactor(challenge: string, code: string): Promise<LoginResponse> {
		return this.loggedIn(
			await this.request<LoginResponse>('POST', '/auth/login/second-factor', { challenge, code })
		);
	}

	async register(username: string, password: string, displayName: string): Promise<LoginResponse> {
		return this.loggedIn(
			await this.request<LoginResponse>('POST', '/auth/register', {
				username,
				password,
				displayName
			})
		);
	}

	private loggedIn(r: LoginResponse): LoginResponse {
		if (r.status === 'ok') this.csrfToken = r.csrfToken;
		return r;
	}

	async loginWithPasskey(): Promise<LoginResponse> {
		const start = await this.request<{ challengeId: string; options: JsonOptions }>(
			'POST',
			'/auth/passkey/start',
			{}
		);
		const credential = await getPasskey(start.options);
		return this.loggedIn(
			await this.request<LoginResponse>('POST', '/auth/passkey/finish', {
				challengeId: start.challengeId,
				credential
			})
		);
	}

	async loginSecondFactorPasskey(challenge: string): Promise<LoginResponse> {
		const start = await this.request<{ challengeId: string; options: JsonOptions }>(
			'POST',
			'/auth/login/second-factor/passkey/start',
			{ challenge }
		);
		const credential = await getPasskey(start.options);
		return this.loggedIn(
			await this.request<LoginResponse>('POST', '/auth/login/second-factor/passkey/finish', {
				challengeId: start.challengeId,
				credential,
				challenge
			})
		);
	}

	passkeys(): Promise<PasskeyInfo[]> {
		return this.request<PasskeyInfo[]>('GET', '/me/passkeys');
	}

	async addPasskey(password: string, name: string): Promise<PasskeyInfo> {
		const start = await this.request<{ challengeId: string; options: JsonOptions }>(
			'POST',
			'/me/passkeys/register/start',
			{ password, name }
		);
		const credential = await createPasskey(start.options);
		return this.request<PasskeyInfo>('POST', '/me/passkeys/register/finish', {
			challengeId: start.challengeId,
			credential
		});
	}

	removePasskey(id: string): Promise<void> {
		return this.request<void>('DELETE', `/me/passkeys/${encodeURIComponent(id)}`);
	}

	async logout(): Promise<void> {
		await this.request<void>('POST', '/auth/logout', {});
		this.csrfToken = null;
	}

	me(): Promise<Me> {
		return this.request<Me>('GET', '/me');
	}

	updateMe(displayName: string): Promise<void> {
		return this.request<void>('PATCH', '/me', { displayName });
	}

	changePassword(
		currentPassword: string,
		newPassword: string,
		disconnectDevices: boolean
	): Promise<void> {
		return this.request<void>('POST', '/me/password', {
			currentPassword,
			newPassword,
			disconnectDevices
		});
	}

	totpSetup(): Promise<TotpSetup> {
		return this.request<TotpSetup>('POST', '/me/totp/setup', {});
	}

	totpConfirm(code: string): Promise<RecoveryCodes> {
		return this.request<RecoveryCodes>('POST', '/me/totp/confirm', { code });
	}

	totpDisable(password: string): Promise<void> {
		return this.request<void>('DELETE', '/me/totp', { password });
	}

	regenerateRecoveryCodes(password: string): Promise<RecoveryCodes> {
		return this.request<RecoveryCodes>('POST', '/me/recovery-codes', { password });
	}

	browserSessions(): Promise<BrowserSession[]> {
		return this.request<BrowserSession[]>('GET', '/me/sessions');
	}

	endBrowserSession(id: string): Promise<void> {
		return this.request<void>('DELETE', `/me/sessions/${encodeURIComponent(id)}`);
	}

	devices(): Promise<DeviceInfo[]> {
		return this.request<DeviceInfo[]>('GET', '/me/devices');
	}

	createDevice(name: string, scope: TokenScope = 'sync'): Promise<NewDevice> {
		return this.request<NewDevice>('POST', '/me/devices', { name, scope });
	}

	revokeDevice(id: string): Promise<void> {
		return this.request<void>('DELETE', `/me/devices/${encodeURIComponent(id)}`);
	}

	approveDeviceLogin(request: {
		redirectUri: string;
		codeChallenge: string;
		state: string;
		deviceName: string;
	}): Promise<{ redirectTo: string }> {
		return this.request<{ redirectTo: string }>('POST', '/device-login/approve', request);
	}

	oauthAuthorizeInfo(query: string): Promise<OAuthAuthorizeInfo> {
		return this.request<OAuthAuthorizeInfo>('GET', `/oauth/authorize${query}`);
	}

	oauthApprove(params: Record<string, string>, allow: boolean): Promise<{ redirectTo: string }> {
		return this.request<{ redirectTo: string }>('POST', '/oauth/approve', { params, allow });
	}

	adminUsers(): Promise<AdminUser[]> {
		return this.request<AdminUser[]>('GET', '/admin/users');
	}

	adminCreateUser(
		username: string,
		displayName: string,
		isAdmin: boolean
	): Promise<TemporaryPassword> {
		return this.request<TemporaryPassword>('POST', '/admin/users', {
			username,
			displayName,
			isAdmin
		});
	}

	adminUpdateUser(id: string, update: UserUpdate): Promise<TemporaryPassword> {
		return this.request<TemporaryPassword>(
			'PATCH',
			`/admin/users/${encodeURIComponent(id)}`,
			update
		);
	}

	adminSettings(): Promise<ServerSettings> {
		return this.request<ServerSettings>('GET', '/admin/settings');
	}

	adminSaveSettings(settings: ServerSettings): Promise<ServerSettings> {
		return this.request<ServerSettings>('PUT', '/admin/settings', settings);
	}

	adminAudit(limit = 200): Promise<AuditEntry[]> {
		return this.request<AuditEntry[]>('GET', `/admin/audit?limit=${limit}`);
	}

	onUnauthorized(handler: () => void): void {
		this.unauthorized = handler;
	}

	// ----- transport -----

	private async request<T>(method: string, path: string, body?: unknown): Promise<T> {
		const headers: Record<string, string> = { Accept: 'application/json' };
		let payload: BodyInit | undefined;
		if (body instanceof Blob) {
			headers['Content-Type'] = 'application/octet-stream';
			payload = body;
		} else if (body !== undefined) {
			headers['Content-Type'] = 'application/json';
			payload = JSON.stringify(body);
		}
		if (method !== 'GET' && this.csrfToken) headers['X-CSRF-Token'] = this.csrfToken;
		let response: Response;
		try {
			response = await this.fetchFn(this.baseUrl + path, {
				method,
				headers,
				body: payload,
				credentials: 'same-origin'
			});
		} catch (e) {
			throw new ApiError(`Server not reachable: ${String(e)}`);
		}
		if (!response.ok) {
			const error = await errorFrom(response);
			// a lost session, but not a failed login attempt
			if (response.status === 401 && !path.startsWith('/auth/')) this.unauthorized?.();
			throw error;
		}
		if (response.status === 204) return undefined as T;
		return (await response.json()) as T;
	}
}

/** The server's `{error, code}` body, or the status text. */
async function errorFrom(response: Response): Promise<ApiError> {
	let message = `${response.status} ${response.statusText}`;
	let code: string | undefined;
	try {
		const body = (await response.json()) as { error?: string; code?: string };
		if (body.error) message = body.error;
		code = body.code;
	} catch {
		// not JSON (e.g. a proxy error page)
	}
	return new ApiError(message, response.status, code);
}

/** Opens the browser's file picker; resolves with nothing if cancelled. */
function pickFiles(accept: string): Promise<File[]> {
	return new Promise((resolve) => {
		const input = document.createElement('input');
		input.type = 'file';
		input.multiple = true;
		input.accept = accept;
		input.addEventListener('change', () => resolve(Array.from(input.files ?? [])));
		input.addEventListener('cancel', () => resolve([]));
		input.click();
	});
}
