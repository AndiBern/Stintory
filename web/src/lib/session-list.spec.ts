// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from 'vitest';
import type { SessionSummary } from './api/index.ts';
import {
	DEFAULT_SORT,
	NO_FILTER,
	filterOptions,
	filterSessions,
	isFiltered,
	nextSort,
	pageForRow,
	pageNumbers,
	paginate,
	sortSessions
} from './session-list.ts';

function session(id: string, over: Partial<SessionSummary>): SessionSummary {
	return {
		id,
		game: 'iracing',
		trackKey: 'spa',
		trackName: 'Spa',
		carKey: 'gt3',
		carName: 'GT3',
		sessionType: 'practice',
		startedAt: new Date(2026, 10, 1, 12).toISOString(),
		lapCount: 5,
		bestLapMs: 140_000,
		origin: { kind: 'own' },
		...over
	};
}

const list = [
	session('a', { startedAt: new Date(2026, 10, 1, 23, 30).toISOString() }),
	session('b', {
		game: 'lmu',
		trackName: 'Monza',
		carName: 'Hypercar',
		sessionType: 'race',
		startedAt: new Date(2026, 10, 3, 0, 15).toISOString(),
		lapCount: 20,
		bestLapMs: 95_000
	}),
	session('c', {
		game: 'f1_25',
		trackName: 'monaco',
		carName: 'F1',
		sessionType: 'qualifying',
		startedAt: new Date(2026, 10, 2, 10).toISOString(),
		lapCount: 8,
		bestLapMs: null
	})
];
/** Plus a copy of a session someone shared and an imported one. */
const withOwners = [
	...list,
	session('d', { origin: { kind: 'synced', server: 'https://example.org', owner: 'friend' } }),
	session('e', { origin: { kind: 'imported', driverName: 'Someone' } })
];
const ids = (l: SessionSummary[]) => l.map((s) => s.id);

describe('filterSessions', () => {
	it('shows only the own sessions by default, imported ones included', () => {
		expect(ids(filterSessions(withOwners, NO_FILTER))).toEqual(['a', 'b', 'c', 'e']);
		expect(isFiltered(NO_FILTER)).toBe(false);
	});

	it('filters by owner', () => {
		expect(ids(filterSessions(withOwners, { ...NO_FILTER, owner: 'shared' }))).toEqual(['d']);
		expect(ids(filterSessions(withOwners, { ...NO_FILTER, owner: 'all' }))).toHaveLength(5);
		expect(isFiltered({ ...NO_FILTER, owner: 'all' })).toBe(true);
	});

	it('filters by game, track, car and type', () => {
		expect(ids(filterSessions(list, { ...NO_FILTER, game: 'lmu' }))).toEqual(['b']);
		expect(ids(filterSessions(list, { ...NO_FILTER, track: 'Spa' }))).toEqual(['a']);
		expect(ids(filterSessions(list, { ...NO_FILTER, car: 'F1' }))).toEqual(['c']);
		expect(ids(filterSessions(list, { ...NO_FILTER, type: 'race' }))).toEqual(['b']);
		expect(ids(filterSessions(list, { ...NO_FILTER, game: 'lmu', type: 'practice' }))).toEqual([]);
	});

	it('filters by local calendar day, both ends inclusive', () => {
		const range = { ...NO_FILTER, from: '2026-11-01', to: '2026-11-02' };
		expect(ids(filterSessions(list, range))).toEqual(['a', 'c']);
		expect(ids(filterSessions(list, { ...NO_FILTER, from: '2026-11-03' }))).toEqual(['b']);
		expect(ids(filterSessions(list, { ...NO_FILTER, to: '2026-11-01' }))).toEqual(['a']);
	});
});

describe('filterOptions', () => {
	it('offers the tracks and cars of the other filters, sorted', () => {
		expect(filterOptions(list, NO_FILTER).tracks).toEqual(['monaco', 'Monza', 'Spa']);
		expect(filterOptions(list, { ...NO_FILTER, game: 'lmu' })).toEqual({
			tracks: ['Monza'],
			cars: ['Hypercar']
		});
		// the chosen track doesn't narrow the track list itself
		expect(filterOptions(list, { ...NO_FILTER, track: 'Spa' }).tracks).toHaveLength(3);
	});
});

describe('sortSessions', () => {
	it('sorts newest first by default', () => {
		expect(ids(sortSessions(list, DEFAULT_SORT))).toEqual(['b', 'c', 'a']);
	});

	it('sorts text case-insensitively', () => {
		expect(ids(sortSessions(list, { key: 'track', descending: false }))).toEqual(['c', 'b', 'a']);
	});

	it('sorts types in their natural order', () => {
		expect(ids(sortSessions(list, { key: 'type', descending: false }))).toEqual(['a', 'c', 'b']);
	});

	it('puts sessions without a best lap last in both directions', () => {
		expect(ids(sortSessions(list, { key: 'best', descending: false }))).toEqual(['b', 'a', 'c']);
		expect(ids(sortSessions(list, { key: 'best', descending: true }))).toEqual(['a', 'b', 'c']);
	});

	it('breaks ties newest first', () => {
		const tie = [session('old', {}), session('new', { startedAt: '2026-12-01T10:00:00Z' })];
		expect(ids(sortSessions(tie, { key: 'track', descending: false }))).toEqual(['new', 'old']);
	});
});

describe('nextSort', () => {
	it('flips the same column and starts new ones in their natural order', () => {
		expect(nextSort(DEFAULT_SORT, 'date')).toEqual({ key: 'date', descending: false });
		expect(nextSort(DEFAULT_SORT, 'track')).toEqual({ key: 'track', descending: false });
		expect(nextSort(DEFAULT_SORT, 'laps')).toEqual({ key: 'laps', descending: true });
	});
});

describe('paginate', () => {
	const items = Array.from({ length: 95 }, (_, i) => i);

	it('cuts out the page', () => {
		expect(paginate(items, 2, 20)).toEqual({
			items: items.slice(20, 40),
			page: 2,
			pageCount: 5,
			start: 20
		});
		expect(paginate(items, 5, 20).items).toEqual(items.slice(80));
	});

	it('clamps a page out of range', () => {
		expect(paginate(items, 9, 20).page).toBe(5);
		expect(paginate(items, 0, 20).page).toBe(1);
		expect(paginate(items, NaN, 20).page).toBe(1);
	});

	it('has one empty page for an empty list', () => {
		expect(paginate([], 3, 20)).toEqual({ items: [], page: 1, pageCount: 1, start: 0 });
	});
});

describe('pageNumbers', () => {
	it('shows all pages when there are few', () => {
		expect(pageNumbers(1, 1)).toEqual([1]);
		expect(pageNumbers(2, 5)).toEqual([1, 2, 3, 4, 5]);
	});

	it('leaves gaps around the current page', () => {
		expect(pageNumbers(6, 12)).toEqual([1, null, 5, 6, 7, null, 12]);
		expect(pageNumbers(1, 12)).toEqual([1, 2, null, 12]);
		expect(pageNumbers(12, 12)).toEqual([1, null, 11, 12]);
	});

	it('shows a single page instead of a gap', () => {
		expect(pageNumbers(4, 12)).toEqual([1, 2, 3, 4, 5, null, 12]);
	});
});

describe('pageForRow', () => {
	it('keeps the first shown row on screen', () => {
		expect(pageForRow(0, 50)).toBe(1);
		expect(pageForRow(50, 25)).toBe(3);
		expect(pageForRow(75, 50)).toBe(2);
	});
});
