// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

/** Filtering and sorting of the sessions table. */

import type { SessionSummary } from './api/index.ts';
import { GAMES, SESSION_TYPES } from './format.ts';
import { loadFlag, loadText, saveFlag, saveText } from './view-prefs.ts';

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

export const SORT_KEYS = ['date', 'track', 'car', 'type', 'laps', 'best'] as const;
export type SortKey = (typeof SORT_KEYS)[number];

export interface Sort {
	key: SortKey;
	descending: boolean;
}

export const DEFAULT_SORT: Sort = { key: 'date', descending: true };

const OWNERS: OwnerFilter[] = ['mine', 'shared', 'all'];
const DAY = /^\d{4}-\d{2}-\d{2}$/;

/** `hasOwn`, not `in`: "toString" or "__proto__" are no game. */
const anyOrKey = (names: Record<string, string>) => (v: string) =>
	v === '' || Object.hasOwn(names, v);
const anyOrDay = (v: string) => v === '' || (DAY.test(v) && !Number.isNaN(Date.parse(v)));

/** What a stored filter value may be; anything else falls back to "any". */
const VALID_FILTER: Record<keyof SessionFilter, (v: string) => boolean> = {
	owner: (v) => (OWNERS as string[]).includes(v),
	game: anyOrKey(GAMES),
	// names of tracks and cars no session has are dropped by `withoutUnknownNames`
	track: () => true,
	car: () => true,
	type: anyOrKey(SESSION_TYPES),
	from: anyOrDay,
	to: anyOrDay
};

const FILTER_KEY = 'sessions.filter.';

/** The filter of the last visit, checked value by value. */
export function loadFilter(): SessionFilter {
	const f = { ...NO_FILTER };
	for (const k of Object.keys(NO_FILTER) as (keyof SessionFilter)[]) {
		(f[k] as string) = loadText(FILTER_KEY + k, NO_FILTER[k], VALID_FILTER[k]);
	}
	if (f.from && f.to && f.from > f.to) f.to = '';
	return f;
}

/** Remembers the filter for the next visit. */
export function saveFilter(f: SessionFilter): void {
	for (const k of Object.keys(NO_FILTER) as (keyof SessionFilter)[]) {
		saveText(FILTER_KEY + k, f[k]);
	}
}

export const PAGE_SIZES = [25, 50, 100] as const;
export const DEFAULT_PAGE_SIZE = 50;

/** What the sessions table shows: filter, sort order, page and page size. */
export interface SessionsView {
	filter: SessionFilter;
	sort: Sort;
	/**
	 * 1-based; one past the end shows the last page (see `paginate`). Not
	 * stored: a new start begins on page 1.
	 */
	page: number;
	size: number;
}

const VIEW_KEY = 'sessions.';
const isSortKey = (v: string) => (SORT_KEYS as readonly string[]).includes(v);
const isPageSize = (v: string) => PAGE_SIZES.some((n) => String(n) === v);

/** The table as it was left on the last visit, checked value by value. */
export function loadView(): SessionsView {
	return {
		filter: loadFilter(),
		sort: {
			key: loadText(VIEW_KEY + 'sort.key', DEFAULT_SORT.key, isSortKey) as SortKey,
			descending: loadFlag(VIEW_KEY + 'sort.descending', DEFAULT_SORT.descending)
		},
		page: 1,
		size: Number(loadText(VIEW_KEY + 'size', String(DEFAULT_PAGE_SIZE), isPageSize))
	};
}

/** Remembers the table for the next visit. */
export function saveView(v: SessionsView): void {
	saveFilter(v.filter);
	saveText(VIEW_KEY + 'sort.key', v.sort.key);
	saveFlag(VIEW_KEY + 'sort.descending', v.sort.descending);
	saveText(VIEW_KEY + 'size', String(v.size));
}

/**
 * Drops a track or car no session has (any more), so a remembered filter
 * never hides every session. Returns `f` itself when nothing changed.
 */
export function withoutUnknownNames(f: SessionFilter, list: SessionSummary[]): SessionFilter {
	const track = f.track && !list.some((s) => s.trackName === f.track) ? '' : f.track;
	const car = f.car && !list.some((s) => s.carName === f.car) ? '' : f.car;
	return track === f.track && car === f.car ? f : { ...f, track, car };
}

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

/** The session passes the filter. */
export function matchesFilter(s: SessionSummary, f: SessionFilter): boolean {
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
}

export function filterSessions(list: SessionSummary[], f: SessionFilter): SessionSummary[] {
	return list.filter((s) => matchesFilter(s, f));
}

/** The start of the newest session, '' for none. */
export function newestStart(list: SessionSummary[]): string {
	let newest = '';
	for (const s of list) {
		if (!newest || Date.parse(s.startedAt) > Date.parse(newest)) newest = s.startedAt;
	}
	return newest;
}

/**
 * Sessions that started after `seenUntil` and that the filter hides, but
 * clearing the filters would show: the ones to point out after a restart or a
 * recording while filtered.
 */
export function hiddenNewSessions(
	list: SessionSummary[],
	f: SessionFilter,
	seenUntil: string
): SessionSummary[] {
	const after = Date.parse(seenUntil);
	if (Number.isNaN(after)) return [];
	return list.filter(
		(s) => Date.parse(s.startedAt) > after && matchesFilter(s, NO_FILTER) && !matchesFilter(s, f)
	);
}

const SEEN_KEY = 'sessions.seenUntil';
const isTimestamp = (v: string) =>
	/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$/.test(v) &&
	!Number.isNaN(Date.parse(v));

/** The start of the newest session the user has seen, '' before the first visit. */
export function loadSeenUntil(): string {
	return loadText(SEEN_KEY, '', isTimestamp);
}

export function saveSeenUntil(startedAt: string): void {
	saveText(SEEN_KEY, startedAt);
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

export interface Page<T> {
	items: T[];
	/** 1-based, clamped to the pages there are */
	page: number;
	/** At least 1, so an empty list still has a page */
	pageCount: number;
	/** 0-based index of the first item, for "21–40 of 95" */
	start: number;
}

/** One page of a list; a page past the end (e.g. after deleting) shows the last one. */
export function paginate<T>(list: T[], page: number, size: number): Page<T> {
	const pageCount = Math.max(1, Math.ceil(list.length / size));
	const current = Math.min(Math.max(1, Math.floor(page) || 1), pageCount);
	const start = (current - 1) * size;
	return { items: list.slice(start, start + size), page: current, pageCount, start };
}

/**
 * The page buttons to show: first, last and the neighbours of the current
 * page, with `null` for a gap, e.g. `1 … 4 5 6 … 12`. A gap of one page shows
 * that page instead.
 */
export function pageNumbers(page: number, pageCount: number): (number | null)[] {
	const shown = new Set([1, pageCount, page - 1, page, page + 1]);
	const out: (number | null)[] = [];
	for (let p = 1; p <= pageCount; p++) {
		const near = shown.has(p) || (shown.has(p - 1) && shown.has(p + 1));
		if (near) out.push(p);
		else if (out.at(-1) !== null) out.push(null);
	}
	return out;
}

/** After changing the page size: the page that still has the first row that was shown. */
export function pageForRow(start: number, size: number): number {
	return Math.floor(start / size) + 1;
}
