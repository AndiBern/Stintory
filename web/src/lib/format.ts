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
