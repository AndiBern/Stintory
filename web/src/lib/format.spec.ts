// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from 'vitest';
import {
	fileName,
	formatBytes,
	formatDelta,
	formatLapTime,
	formatOrigin,
	formatSessionType
} from './format.ts';

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

describe('formatBytes', () => {
	it('picks a readable unit', () => {
		expect(formatBytes(0)).toBe('0 B');
		expect(formatBytes(1536)).toBe('1.5 KB');
		expect(formatBytes(19_198_000)).toBe('18.3 MB');
	});
});

describe('formatSessionType', () => {
	it('maps known types and keeps unknown ones', () => {
		expect(formatSessionType('time_trial')).toBe('Time trial');
		expect(formatSessionType('hotlap')).toBe('hotlap');
	});
});

describe('formatOrigin', () => {
	it('labels imported and shared sessions', () => {
		expect(formatOrigin({ kind: 'own' })).toBe('');
		expect(formatOrigin({ kind: 'imported', driverName: null })).toBe('Imported');
		expect(formatOrigin({ kind: 'imported', driverName: 'Max' })).toBe('Imported · Max');
		expect(formatOrigin({ kind: 'synced', server: 'https://x', owner: 'max' })).toBe(
			'Shared by max'
		);
	});
});

describe('fileName', () => {
	it('removes characters not allowed in file names', () => {
		expect(fileName('Winton – National', 'Mazda MX-5 Cup', '2026-11-10 17-00')).toBe(
			'Winton – National Mazda MX-5 Cup 2026-11-10 17-00'
		);
		expect(fileName('a/b:c*?', 'd.')).toBe('abc d');
		expect(fileName('???')).toBe('export');
	});
});
