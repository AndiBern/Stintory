// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { AppInfo, SessionSummary } from './types.ts';

/**
 * The one API shape the frontend talks to.
 *
 * - In the desktop app it is implemented by {@link TauriApi} (Tauri `invoke`).
 * - In the browser it is implemented by {@link HttpApi} (rz-server REST API).
 *
 * Views must only use this interface, never a concrete implementation.
 */
export interface ApiClient {
	getInfo(): Promise<AppInfo>;
	listSessions(): Promise<SessionSummary[]>;
}

/** Error raised by any ApiClient implementation. */
export class ApiError extends Error {
	constructor(
		message: string,
		readonly status?: number
	) {
		super(message);
		this.name = 'ApiError';
	}
}
