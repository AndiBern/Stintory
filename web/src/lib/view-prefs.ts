// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * View choices remembered across visits, such as a collapsed section. They
 * live in the browser (or the desktop app's webview), not in the settings:
 * they only change how a page looks. Storage can be unavailable (private
 * windows, blocked site data); the default is used then.
 */

const PREFIX = 'stintory.view.';

/** A remembered yes/no choice, or `fallback` if none was stored. */
export function loadFlag(key: string, fallback: boolean): boolean {
	try {
		const v = localStorage.getItem(PREFIX + key);
		return v === null ? fallback : v === '1';
	} catch {
		return fallback;
	}
}

/** Remembers a yes/no choice. */
export function saveFlag(key: string, value: boolean): void {
	try {
		localStorage.setItem(PREFIX + key, value ? '1' : '0');
	} catch {
		// not remembered, still applies for this visit
	}
}
