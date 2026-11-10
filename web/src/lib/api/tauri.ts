// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { invoke } from '@tauri-apps/api/core';
import { ApiError, type ApiClient } from './client.ts';
import type { AppInfo, SessionSummary } from './types.ts';

/** ApiClient backed by the Rust commands of the desktop app (apps/desktop). */
export class TauriApi implements ApiClient {
	getInfo(): Promise<AppInfo> {
		return call<AppInfo>('get_info');
	}

	listSessions(): Promise<SessionSummary[]> {
		return call<SessionSummary[]>('list_sessions');
	}
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
	try {
		return await invoke<T>(command, args);
	} catch (e) {
		throw new ApiError(typeof e === 'string' ? e : String(e));
	}
}
