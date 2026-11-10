// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from 'vitest';
import { formatDelta, formatLapTime } from './format.ts';

describe('formatLapTime', () => {
	it('formats minutes, seconds and milliseconds', () => {
		expect(formatLapTime(83_456)).toBe('1:23.456');
		expect(formatLapTime(5_007)).toBe('0:05.007');
		expect(formatLapTime(600_000)).toBe('10:00.000');
	});

	it('rounds fractional milliseconds', () => {
		expect(formatLapTime(83_455.6)).toBe('1:23.456');
	});

	it('returns a dash for missing or invalid values', () => {
		expect(formatLapTime(null)).toBe('–');
		expect(formatLapTime(undefined)).toBe('–');
		expect(formatLapTime(-1)).toBe('–');
		expect(formatLapTime(Number.NaN)).toBe('–');
	});
});

describe('formatDelta', () => {
	it('adds a sign', () => {
		expect(formatDelta(123)).toBe('+0.123');
		expect(formatDelta(-1250)).toBe('-1.250');
		expect(formatDelta(0)).toBe('+0.000');
	});
});
