// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from 'vitest';
import {
	fileName,
	formatAgo,
	formatAudience,
	formatBytes,
	formatDelta,
	formatDriver,
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

describe('formatAgo', () => {
	it('says how long ago', () => {
		const now = Date.parse('2026-11-10T18:00:00Z');
		expect(formatAgo(null, now)).toBe('never');
		expect(formatAgo('2026-11-10T17:59:30Z', now)).toBe('just now');
		expect(formatAgo('2026-11-10T17:55:00Z', now)).toBe('5 min ago');
		expect(formatAgo('2026-11-10T15:00:00Z', now)).toBe('3 h ago');
	});
});

describe('formatAudience', () => {
	it('summarises who can see something', () => {
		expect(formatAudience([])).toBe('Only you');
		expect(
			formatAudience([
				{ kind: 'team', id: 't', name: 'Sim Racers' },
				{ kind: 'user', id: 'u', username: 'max', displayName: 'Max M.' },
				{ kind: 'user', id: 'v', username: 'ben', displayName: 'ben' }
			])
		).toBe('Team Sim Racers, Max M. (@max), @ben');
		expect(
			formatAudience([{ kind: 'team', id: 't', name: 'Sim Racers' }, { kind: 'everyone' }])
		).toBe('Everyone on this server');
	});

	it('says who sees the setups too', () => {
		expect(
			formatAudience([
				{ kind: 'team', id: 't', name: 'Sim Racers', withSetup: true },
				{ kind: 'user', id: 'u', username: 'max', displayName: 'Max M.', withSetup: true },
				{ kind: 'user', id: 'v', username: 'ben', displayName: 'ben', withSetup: false }
			])
		).toBe('Team Sim Racers with setups, Max M. (@max) with setups, @ben');
	});
});

describe('formatDriver', () => {
	it('names who drove a lap', () => {
		const ben = { id: 'b', username: 'ben', displayName: 'Ben B.' };
		const synced = { kind: 'synced', server: 'https://laps.example', owner: 'ben' } as const;
		expect(formatDriver({ owner: null, origin: { kind: 'own' } })).toBe('You');
		expect(formatDriver({ owner: ben, origin: synced })).toBe('Ben B.');
		expect(formatDriver({ owner: { ...ben, displayName: '' }, origin: synced })).toBe('@ben');
		// a kept copy in the desktop app only knows the username
		expect(formatDriver({ owner: null, origin: synced })).toBe('@ben');
		expect(formatDriver({ origin: { kind: 'imported', driverName: 'Max' } })).toBe('Max');
		expect(formatDriver({ origin: { kind: 'imported', driverName: null } })).toBe('Imported');
	});
});
