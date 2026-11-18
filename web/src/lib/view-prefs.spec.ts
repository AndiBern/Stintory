// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { afterEach, describe, expect, it, vi } from 'vitest';
import { loadFlag, saveFlag } from './view-prefs.ts';

function memoryStorage(): Storage {
	const m = new Map<string, string>();
	return {
		get length() {
			return m.size;
		},
		clear: () => m.clear(),
		getItem: (k) => m.get(k) ?? null,
		key: (i) => [...m.keys()][i] ?? null,
		removeItem: (k) => void m.delete(k),
		setItem: (k, v) => void m.set(k, String(v))
	};
}

afterEach(() => {
	vi.unstubAllGlobals();
});

describe('view prefs', () => {
	it('uses the default until something is stored', () => {
		vi.stubGlobal('localStorage', memoryStorage());
		expect(loadFlag('x', true)).toBe(true);
		expect(loadFlag('x', false)).toBe(false);
	});

	it('remembers a choice', () => {
		vi.stubGlobal('localStorage', memoryStorage());
		saveFlag('x', false);
		expect(loadFlag('x', true)).toBe(false);
		saveFlag('x', true);
		expect(loadFlag('x', false)).toBe(true);
	});

	it('falls back to the default when storage is blocked', () => {
		const blocked = memoryStorage();
		blocked.getItem = () => {
			throw new Error('blocked');
		};
		blocked.setItem = () => {
			throw new Error('blocked');
		};
		vi.stubGlobal('localStorage', blocked);
		expect(() => saveFlag('x', false)).not.toThrow();
		expect(loadFlag('x', true)).toBe(true);
	});
});
