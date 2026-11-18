// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * View choices remembered across visits, such as a collapsed section. They
 * live in the browser (or the desktop app's webview), not in the settings:
 * they only change how a page looks. Storage can be unavailable (private
 * windows, blocked site data); the default is used then.
 */

const PREFIX = 'stintory.view.';
/** Longer stored texts are not taken, whatever they are. */
const MAX_TEXT = 500;

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

/**
 * A remembered text, or `fallback` if none was stored or `valid` rejects it.
 * Anyone can edit what is stored, so `valid` must accept only what the page
 * could have saved itself.
 */
export function loadText(key: string, fallback: string, valid: (v: string) => boolean): string {
	try {
		const v = localStorage.getItem(PREFIX + key);
		return v !== null && v.length <= MAX_TEXT && valid(v) ? v : fallback;
	} catch {
		return fallback;
	}
}

/** Remembers a text. */
export function saveText(key: string, value: string): void {
	try {
		localStorage.setItem(PREFIX + key, value);
	} catch {
		// not remembered, still applies for this visit
	}
}
