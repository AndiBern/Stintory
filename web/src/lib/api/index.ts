// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ApiClient } from './client.ts';
import { HttpApi, type AccountApi } from './http.ts';
import { TauriApi } from './tauri.ts';

export type { ApiClient } from './client.ts';
export type { AccountApi } from './http.ts';
export { ApiError } from './client.ts';
export type * from './types.ts';

/** True when running inside the Tauri desktop app. */
export function isTauri(): boolean {
	return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

let instance: ApiClient | undefined;

/** The ApiClient for the current runtime (desktop app or browser). */
export function api(): ApiClient {
	instance ??= isTauri() ? new TauriApi() : new HttpApi();
	return instance;
}

/** Server account features, or null in the desktop app. */
export function accountApi(): AccountApi | null {
	const client = api();
	return client instanceof HttpApi ? client : null;
}
