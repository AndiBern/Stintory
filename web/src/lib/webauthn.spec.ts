// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from 'vitest';
import { fromBase64url, toBase64url } from './webauthn.ts';

describe('base64url', () => {
	it('round-trips binary data without padding', () => {
		for (const len of [0, 1, 2, 3, 31, 32, 33]) {
			const bytes = new Uint8Array(len).map((_, i) => (i * 37 + 250) % 256);
			const text = toBase64url(bytes);
			expect(text).not.toMatch(/[+/=]/);
			expect(new Uint8Array(fromBase64url(text))).toEqual(bytes);
		}
	});

	it('matches RFC 4648 examples', () => {
		expect(toBase64url(new TextEncoder().encode('foobar'))).toBe('Zm9vYmFy');
		expect(toBase64url(new Uint8Array([0xfb, 0xff]))).toBe('-_8');
	});
});
