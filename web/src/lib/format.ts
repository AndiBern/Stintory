// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

/** Formats a lap time in milliseconds as `m:ss.mmm` (e.g. `1:23.456`). */
export function formatLapTime(ms: number | null | undefined): string {
	if (ms === null || ms === undefined || !Number.isFinite(ms) || ms < 0) {
		return '–';
	}
	const total = Math.round(ms);
	const minutes = Math.floor(total / 60_000);
	const seconds = Math.floor((total % 60_000) / 1000);
	const millis = total % 1000;
	return `${minutes}:${String(seconds).padStart(2, '0')}.${String(millis).padStart(3, '0')}`;
}

/** Formats a time delta in milliseconds with sign, e.g. `+0.123` / `-1.250`. */
export function formatDelta(ms: number): string {
	const sign = ms < 0 ? '-' : '+';
	return `${sign}${(Math.abs(ms) / 1000).toFixed(3)}`;
}

const SESSION_TYPES: Record<string, string> = {
	practice: 'Practice',
	qualifying: 'Qualifying',
	race: 'Race',
	time_trial: 'Time trial',
	other: 'Other'
};

/** Human-readable session type. */
export function formatSessionType(type: string): string {
	return SESSION_TYPES[type] ?? type;
}

const GAMES: Record<string, string> = {
	iracing: 'iRacing',
	f1_25: 'F1 25',
	lmu: 'Le Mans Ultimate'
};

/** Human-readable game name. */
export function formatGame(game: string): string {
	return GAMES[game] ?? game;
}

/** Date and time in the user's locale, e.g. `9.11.2026, 14:27`. */
export function formatDateTime(iso: string | null | undefined): string {
	if (!iso) return '–';
	const d = new Date(iso);
	return Number.isNaN(d.getTime())
		? '–'
		: d.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

/** Byte size, e.g. `18.3 MB`. */
export function formatBytes(bytes: number): string {
	if (!Number.isFinite(bytes) || bytes < 0) return '–';
	const units = ['B', 'KB', 'MB', 'GB', 'TB'];
	let value = bytes;
	let unit = 0;
	while (value >= 1024 && unit < units.length - 1) {
		value /= 1024;
		unit++;
	}
	return `${value.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}
