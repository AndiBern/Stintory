// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * Where each lap's car is on the comparison page. All laps share one
 * distance grid (`CompareData.distM`); the cursor is a grid index of the
 * reference lap (lap 0).
 *
 * - `distance`: every car at the same distance as the reference: shows the
 *   lines side by side.
 * - `time`: every car where it was after the same lap time as the
 *   reference: shows the gaps like on track.
 */

import type { CompareData, LapTrace } from '#lib/api/index.ts';

export type OffsetMode = 'distance' | 'time';

type Series = (number | null)[];

/** Linear interpolation at a fractional index; null where data is missing. */
export function valueAt(values: Series | undefined, index: number): number | null {
	if (!values || !(index >= 0) || index > values.length - 1) return null;
	const i = Math.floor(index);
	const a = values[i];
	if (a == null) return null;
	const f = index - i;
	if (f === 0) return a;
	const b = values[i + 1];
	return b == null ? a : a + (b - a) * f;
}

/**
 * The fractional index where `time` (s, rising) reaches `target`, or null if
 * the lap doesn't cover that time.
 */
export function indexAtTime(time: Series | undefined, target: number): number | null {
	if (!time) return null;
	let lo = time.findIndex((t) => t != null);
	let hi = time.length - 1;
	while (hi >= 0 && time[hi] == null) hi--;
	if (lo < 0 || hi < lo || target < time[lo]! || target > time[hi]!) return null;
	// binary search for the last index with time <= target (gaps skipped)
	while (hi - lo > 1) {
		const mid = (lo + hi) >> 1;
		let m = mid;
		while (m < hi && time[m] == null) m++;
		if (m === hi) {
			hi = mid;
			continue;
		}
		if (time[m]! <= target) lo = m;
		else hi = m;
	}
	const a = time[lo]!;
	const b = time[hi];
	if (b == null || b === a) return lo;
	return lo + Math.min(1, Math.max(0, (target - a) / (b - a))) * (hi - lo);
}

/** The reference lap's time at the cursor. */
export function referenceTime(data: CompareData, cursor: number): number | null {
	return valueAt(data.laps[0]?.channels.t, cursor);
}

/** Grid index of lap `k`'s car for the cursor; null if it isn't on track then. */
export function carIndex(
	data: CompareData,
	k: number,
	cursor: number,
	mode: OffsetMode
): number | null {
	if (k === 0 || mode === 'distance') return cursor;
	const t = referenceTime(data, cursor);
	return t == null ? null : indexAtTime(data.laps[k].channels.t, t);
}

export interface Point {
	x: number;
	y: number;
}

/** Position (m, x east, y north) of a lap at a fractional index. */
export function positionAt(lap: LapTrace, index: number): Point | null {
	const x = valueAt(lap.channels.pos_x, index);
	const y = valueAt(lap.channels.pos_y, index);
	return x == null || y == null ? null : { x, y };
}

/**
 * Driving direction (radians, counter-clockwise from east) at an index,
 * from the positions a few metres before and after (steadier than the
 * neighbours).
 */
export function headingAt(lap: LapTrace, index: number, stepM: number, spanM = 4): number | null {
	const span = Math.max(1, Math.round(spanM / stepM));
	const a = positionAt(lap, Math.max(0, index - span)) ?? positionAt(lap, index);
	const b = positionAt(lap, index + span) ?? positionAt(lap, index);
	if (!a || !b || (a.x === b.x && a.y === b.y)) return null;
	return Math.atan2(b.y - a.y, b.x - a.x);
}

/** Last grid index where the reference lap has data (the slider's end). */
export function lastIndex(data: CompareData): number {
	const t = data.laps[0]?.channels.t ?? [];
	let i = t.length - 1;
	while (i > 0 && t[i] == null) i--;
	return Math.max(0, i);
}

/** First grid index where the reference lap has data. */
export function firstIndex(data: CompareData): number {
	const i = (data.laps[0]?.channels.t ?? []).findIndex((t) => t != null);
	return Math.max(0, i);
}

/**
 * Gap of lap `k` to the reference for the map's list: time at the same
 * distance (`+0.231 s` = slower), or distance at the same time (`12 m
 * behind`).
 */
export function gapText(data: CompareData, k: number, cursor: number, mode: OffsetMode): string {
	if (k === 0) return 'reference';
	if (mode === 'distance') {
		const d = valueAt(data.laps[k].deltaS, cursor);
		return d == null ? '–' : `${d >= 0 ? '+' : '−'}${Math.abs(d).toFixed(3)} s`;
	}
	const i = carIndex(data, k, cursor, 'time');
	if (i == null) return '–';
	const m = Math.round((i - cursor) * data.stepM);
	if (m === 0) return 'level';
	return m > 0 ? `${m} m ahead` : `${-m} m behind`;
}
