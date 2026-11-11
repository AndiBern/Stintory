// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from 'vitest';
import type { ReferenceLap } from '#lib/api/index.ts';
import { inScope, isShared } from './ReferenceLapTable.svelte';

function lap(id: string, partial: Partial<ReferenceLap>): ReferenceLap {
	return {
		lapId: id,
		sessionId: 's',
		number: 1,
		lapTimeMs: 90_000,
		sectorTimesMs: [],
		game: 'iracing',
		trackKey: 'iracing:1',
		trackName: 'T',
		carKey: 'iracing:2',
		carName: 'C',
		sessionType: 'practice',
		sessionStartedAt: '2026-11-10T18:00:00Z',
		origin: { kind: 'own' },
		owner: null,
		remote: false,
		...partial
	};
}

describe('reference lap scopes', () => {
	const own = lap('own', {});
	const imported = lap('imp', { origin: { kind: 'imported', driverName: 'Max' } });
	const shared = lap('shared', {
		origin: { kind: 'synced', server: 'https://laps.example', owner: 'ben' },
		owner: { id: 'b', username: 'ben', displayName: 'Ben' }
	});
	// desktop app offline: a kept copy without owner details
	const kept = lap('kept', {
		origin: { kind: 'synced', server: 'https://laps.example', owner: 'ben' }
	});
	const laps = [shared, own, kept, imported];

	it('tells own laps from shared ones', () => {
		expect(laps.map(isShared)).toEqual([true, false, true, false]);
		expect(inScope(laps, 'all')).toEqual(laps);
		expect(inScope(laps, 'mine').map((l) => l.lapId)).toEqual(['own', 'imp']);
		expect(inScope(laps, 'shared').map((l) => l.lapId)).toEqual(['shared', 'kept']);
	});
});
