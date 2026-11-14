// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The corner list of the comparison page: one row per corner with the time
 * a lap loses to the reference there and, in words, what it does
 * differently (braking, apex speed, throttle, exit).
 */

import type { CompareData, Corner, CornerLapStats } from '#lib/api/index.ts';

export interface CornerRow {
	corner: Corner;
	/** the lap's statistics */
	lap: CornerLapStats;
	/** the reference lap's statistics */
	ref: CornerLapStats;
	/** what the lap does differently from the reference */
	notes: string[];
}

/** Differences smaller than these are noise, not driving. */
const MIN_DIST_M = 5;
const MIN_SPEED_KMH = 1;

const kmh = (mps: number) => mps * 3.6;

/** One row per corner for lap `k` (0 = the reference), in lap order. */
export function cornerRows(data: CompareData, k: number): CornerRow[] {
	const report = data.corners;
	const lap = report?.laps[k];
	const ref = report?.laps[0];
	if (!report || !lap || !ref) return [];
	return report.corners.map((corner, i) => ({
		corner,
		lap: lap.corners[i],
		ref: ref.corners[i],
		notes: k === 0 ? [] : cornerNotes(lap.corners[i], ref.corners[i])
	}));
}

/** Rows with the most time lost first; rows without a time last. */
export function byTimeLost(rows: CornerRow[]): CornerRow[] {
	return [...rows].sort((a, b) => (b.lap.timeLostS ?? -Infinity) - (a.lap.timeLostS ?? -Infinity));
}

function metres(d: number): string {
	return `${Math.round(Math.abs(d))} m`;
}

/** What `lap` does differently from `ref` in a corner, in driving order. */
export function cornerNotes(lap: CornerLapStats, ref: CornerLapStats): string[] {
	const notes: string[] = [];

	// braking (or lifting where neither brakes)
	if (lap.brakeM != null && ref.brakeM != null) {
		const d = lap.brakeM - ref.brakeM;
		if (Math.abs(d) >= MIN_DIST_M) notes.push(`brakes ${metres(d)} ${d < 0 ? 'earlier' : 'later'}`);
		if (lap.brakeMax != null && ref.brakeMax != null) {
			const p = Math.round((lap.brakeMax - ref.brakeMax) * 100);
			if (Math.abs(p) >= 10)
				notes.push(`${Math.abs(p)} % ${p < 0 ? 'less' : 'more'} brake pressure`);
		}
	} else if (lap.brakeM != null) {
		notes.push('brakes, the reference doesn’t');
	} else if (ref.brakeM != null) {
		notes.push('doesn’t brake');
	} else if (lap.liftM != null && ref.liftM != null) {
		const d = lap.liftM - ref.liftM;
		if (Math.abs(d) >= MIN_DIST_M) notes.push(`lifts ${metres(d)} ${d < 0 ? 'earlier' : 'later'}`);
	} else if (lap.liftM != null && ref.flatOut) {
		notes.push('lifts, the reference is flat out');
	} else if (lap.flatOut && ref.liftM != null) {
		notes.push('flat out, the reference lifts');
	}

	if (lap.minSpeedMps != null && ref.minSpeedMps != null) {
		const d = kmh(lap.minSpeedMps - ref.minSpeedMps);
		if (Math.abs(d) >= MIN_SPEED_KMH) {
			notes.push(`${Math.round(Math.abs(d))} km/h ${d < 0 ? 'slower' : 'faster'} at the apex`);
		}
	}
	if (
		lap.minSpeedGear != null &&
		ref.minSpeedGear != null &&
		lap.minSpeedGear !== ref.minSpeedGear &&
		!lap.flatOut
	) {
		notes.push(`gear ${lap.minSpeedGear} instead of ${ref.minSpeedGear}`);
	}

	if (lap.throttleM != null && ref.throttleM != null) {
		const d = lap.throttleM - ref.throttleM;
		if (Math.abs(d) >= MIN_DIST_M) {
			notes.push(`on the throttle ${metres(d)} ${d < 0 ? 'earlier' : 'later'}`);
		}
	}
	if (lap.fullThrottleM != null && ref.fullThrottleM != null) {
		const d = lap.fullThrottleM - ref.fullThrottleM;
		if (Math.abs(d) >= 2 * MIN_DIST_M) {
			notes.push(`full throttle ${metres(d)} ${d < 0 ? 'earlier' : 'later'}`);
		}
	}

	if (lap.exitSpeedMps != null && ref.exitSpeedMps != null) {
		const d = kmh(lap.exitSpeedMps - ref.exitSpeedMps);
		if (Math.abs(d) >= MIN_SPEED_KMH) {
			notes.push(`${Math.round(Math.abs(d))} km/h ${d < 0 ? 'slower' : 'faster'} on exit`);
		}
	}
	return notes;
}

/**
 * The part of the lap to show for a corner (m): its segment. The last
 * corner's segment wraps around the line; it is shown up to the lap's end.
 */
export function cornerRange(corner: Corner, lapEndM: number): { min: number; max: number } {
	const max = corner.segmentEndM > corner.segmentStartM ? corner.segmentEndM : lapEndM;
	return { min: corner.segmentStartM, max };
}

/** Signed seconds, e.g. `+0.123` / `−0.050`. */
export function formatSeconds(s: number | null | undefined): string {
	if (s == null || !Number.isFinite(s)) return '–';
	return `${s < 0 ? '−' : '+'}${Math.abs(s).toFixed(3)}`;
}

/** Speed in km/h, rounded. */
export function formatKmh(mps: number | null | undefined): string {
	return mps == null ? '–' : `${Math.round(kmh(mps))}`;
}
