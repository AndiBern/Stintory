// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { Grantee, Origin, UserRef } from './api/types.ts';

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

/** Where a session came from, for badges. Empty for own sessions. */
export function formatOrigin(origin: Origin): string {
	switch (origin.kind) {
		case 'own':
			return '';
		case 'imported':
			return origin.driverName ? `Imported · ${origin.driverName}` : 'Imported';
		case 'synced':
			return `Shared by ${origin.owner}`;
	}
}

/** A file name (without extension) safe on Windows and Linux. */
export function fileName(...parts: string[]): string {
	return (
		parts
			.join(' ')
			// eslint-disable-next-line no-control-regex
			.replace(/[\u0000-\u001f<>:"/\\|?*]+/g, '')
			.replace(/\s+/g, ' ')
			.trim()
			.replace(/[. ]+$/, '') || 'export'
	);
}

/** Date for file names: `2026-11-10 17-00` (local time). */
export function fileDate(iso: string): string {
	const d = new Date(iso);
	const p = (n: number) => String(n).padStart(2, '0');
	return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}-${p(d.getMinutes())}`;
}

/** "just now", "5 min ago", "3 h ago", or the date. */
export function formatAgo(iso: string | null | undefined, now = Date.now()): string {
	if (!iso) return 'never';
	const seconds = (now - new Date(iso).getTime()) / 1000;
	if (seconds < 60) return 'just now';
	if (seconds < 3600) return `${Math.floor(seconds / 60)} min ago`;
	if (seconds < 86400) return `${Math.floor(seconds / 3600)} h ago`;
	return formatDateTime(iso);
}

/** One entry of an audience, e.g. `Team Sim Racers` or `Max (@max)`. */
export function formatGrantee(g: Grantee): string {
	switch (g.kind) {
		case 'everyone':
			return 'Everyone on this server';
		case 'team':
			return `Team ${g.name}`;
		case 'user':
			return g.displayName === g.username ? `@${g.username}` : `${g.displayName} (@${g.username})`;
	}
}

/** Who can see something, e.g. `Only you` or `Team A, Max (@max)`. */
export function formatAudience(grants: Grantee[]): string {
	if (grants.length === 0) return 'Only you';
	if (grants.some((g) => g.kind === 'everyone')) return 'Everyone on this server';
	return grants.map(formatGrantee).join(', ');
}

/**
 * Who drove a lap: `You` for the user's own laps, else the owner of a shared
 * lap or the driver named in an imported file.
 */
export function formatDriver(lap: { owner?: UserRef | null; origin: Origin }): string {
	if (lap.owner) return lap.owner.displayName || `@${lap.owner.username}`;
	switch (lap.origin.kind) {
		case 'own':
			return 'You';
		case 'imported':
			return lap.origin.driverName ? lap.origin.driverName : 'Imported';
		case 'synced':
			return `@${lap.origin.owner}`;
	}
}
