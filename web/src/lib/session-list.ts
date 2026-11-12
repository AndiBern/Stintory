// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

/** Filtering and sorting of the sessions table. */

import type { SessionSummary } from './api/index.ts';
import { SESSION_TYPES } from './format.ts';

/**
 * Whose sessions: the user's own (recorded or imported), the copies of sessions
 * others shared that the desktop app keeps, or both.
 */
export type OwnerFilter = 'mine' | 'shared' | 'all';

/** Empty strings mean "any". Dates are local calendar days (`YYYY-MM-DD`), both inclusive. */
export interface SessionFilter {
	owner: OwnerFilter;
	game: string;
	track: string;
	car: string;
	type: string;
	from: string;
	to: string;
}

export const NO_FILTER: SessionFilter = {
	owner: 'mine',
	game: '',
	track: '',
	car: '',
	type: '',
	from: '',
	to: ''
};

export type SortKey = 'date' | 'track' | 'car' | 'type' | 'laps' | 'best';

export interface Sort {
	key: SortKey;
	descending: boolean;
}

export const DEFAULT_SORT: Sort = { key: 'date', descending: true };

/** The local calendar day of a timestamp, e.g. `2026-11-12`. */
export function localDay(iso: string): string {
	const d = new Date(iso);
	if (Number.isNaN(d.getTime())) return '';
	const pad = (n: number) => String(n).padStart(2, '0');
	return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/** Something other than the default filter is set. */
export function isFiltered(f: SessionFilter): boolean {
	return (Object.keys(NO_FILTER) as (keyof SessionFilter)[]).some((k) => f[k] !== NO_FILTER[k]);
}

/** A copy of a session someone else shared. */
export function isSharedCopy(s: SessionSummary): boolean {
	return s.origin.kind === 'synced';
}

export function filterSessions(list: SessionSummary[], f: SessionFilter): SessionSummary[] {
	return list.filter((s) => {
		if (f.owner !== 'all' && isSharedCopy(s) !== (f.owner === 'shared')) return false;
		if (f.game && s.game !== f.game) return false;
		if (f.track && s.trackName !== f.track) return false;
		if (f.car && s.carName !== f.car) return false;
		if (f.type && s.sessionType !== f.type) return false;
		if (f.from || f.to) {
			const day = localDay(s.startedAt);
			if (f.from && day < f.from) return false;
			if (f.to && day > f.to) return false;
		}
		return true;
	});
}

/**
 * The tracks and cars to offer in the filter: those of the sessions that match
 * the other filters, so picking a game only offers that game's tracks.
 */
export function filterOptions(
	list: SessionSummary[],
	f: SessionFilter
): { tracks: string[]; cars: string[] } {
	const distinct = (names: string[]) =>
		[...new Set(names)].sort((a, b) => a.localeCompare(b, undefined, { sensitivity: 'base' }));
	return {
		tracks: distinct(filterSessions(list, { ...f, track: '' }).map((s) => s.trackName)),
		cars: distinct(filterSessions(list, { ...f, car: '' }).map((s) => s.carName))
	};
}

const typeOrder = Object.keys(SESSION_TYPES);
const text = (a: string, b: string) => a.localeCompare(b, undefined, { sensitivity: 'base' });

const compare: Record<SortKey, (a: SessionSummary, b: SessionSummary) => number> = {
	date: (a, b) => Date.parse(a.startedAt) - Date.parse(b.startedAt),
	track: (a, b) => text(a.trackName, b.trackName),
	car: (a, b) => text(a.carName, b.carName),
	type: (a, b) => typeOrder.indexOf(a.sessionType) - typeOrder.indexOf(b.sessionType),
	laps: (a, b) => a.lapCount - b.lapCount,
	best: (a, b) => (a.bestLapMs ?? 0) - (b.bestLapMs ?? 0)
};

/** Sorted copy; ties go newest first, sessions without a best lap always last. */
export function sortSessions(list: SessionSummary[], sort: Sort): SessionSummary[] {
	const sign = sort.descending ? -1 : 1;
	return [...list].sort((a, b) => {
		if (sort.key === 'best' && (a.bestLapMs === null) !== (b.bestLapMs === null)) {
			return a.bestLapMs === null ? 1 : -1;
		}
		return sign * compare[sort.key](a, b) || compare.date(b, a);
	});
}

/** Clicking a column: the same one flips the direction; a new one starts in its natural order. */
export function nextSort(current: Sort, key: SortKey): Sort {
	if (current.key === key) return { key, descending: !current.descending };
	return { key, descending: key === 'date' || key === 'laps' };
}
