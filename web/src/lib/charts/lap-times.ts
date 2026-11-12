// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

/** Scales of the lap time chart on the session page ("Session performance"). */

import type { LapSummary } from '#lib/api/index.ts';

/** A lap that counts: valid, complete, no out- or in-lap. */
export function isClean(l: LapSummary): boolean {
	return l.valid && !l.outLap && !l.inLap && !l.incomplete && l.lapTimeMs !== null;
}

/**
 * Laps outside this factor of the median clean lap sit at the edge: much
 * slower ones (out-laps from the pits, spins) at the top, much faster ones
 * (partial laps, like F1's time trial out-lap from the garage) at the bottom.
 */
const OFF_SCALE_FACTOR = 1.1;

export interface TimeScale {
	/** Domain in ms; laps outside it are off the scale */
	min: number;
	max: number;
	/** Tick positions in ms and the step between them */
	ticks: number[];
	step: number;
}

const TIME_STEPS = [100, 200, 250, 500, 1000, 2000, 5000, 10_000, 15_000, 30_000, 60_000];

function median(values: number[]): number {
	const sorted = [...values].sort((a, b) => a - b);
	const mid = Math.floor(sorted.length / 2);
	return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

/**
 * The time axis. It fits the clean laps (or all timed laps if none is clean)
 * and the laps close to them, so one out-lap doesn't squash the rest.
 * Returns null without timed laps.
 */
export function timeScale(laps: LapSummary[], maxTicks = 5): TimeScale | null {
	const timed = laps.filter((l) => l.lapTimeMs !== null);
	if (timed.length === 0) return null;
	const clean = timed.filter(isClean);
	const reference = (clean.length > 0 ? clean : timed).map((l) => l.lapTimeMs!);
	const mid = median(reference);
	const shown = timed
		.map((l) => l.lapTimeMs!)
		.filter((t) => t <= mid * OFF_SCALE_FACTOR && t >= mid / OFF_SCALE_FACTOR);
	const lo = Math.min(...shown);
	const hi = Math.max(...shown);
	const pad = Math.max((hi - lo) * 0.1, 250);
	const min = lo - pad;
	const max = hi + pad;
	const step = TIME_STEPS.find((s) => (max - min) / s <= maxTicks) ?? 60_000;
	const ticks: number[] = [];
	for (let t = Math.ceil(min / step) * step; t <= max; t += step) ticks.push(t);
	return { min, max, ticks, step };
}

/** Lap number ticks: every lap while they fit, else every 2nd, 5th, 10th… */
export function lapTicks(first: number, last: number, maxTicks: number): number[] {
	const step = [1, 2, 5, 10, 20, 50, 100].find((s) => (last - first) / s < maxTicks) ?? 100;
	const ticks: number[] = [];
	for (let n = Math.ceil(first / step) * step; n <= last; n += step) ticks.push(n);
	if (ticks[0] !== first && step > 1 && ticks[0] - first >= step / 2) ticks.unshift(first);
	return ticks;
}

/** Axis label for a time tick: `1:35` for whole seconds, else `1:35.5`. */
export function formatTick(ms: number, step: number): string {
	const total = Math.round(ms);
	const minutes = Math.floor(total / 60_000);
	const seconds = (total % 60_000) / 1000;
	const digits = step >= 1000 ? 0 : step % 1000 === 250 || step < 200 ? 2 : 1;
	const text = seconds.toFixed(digits).padStart(digits ? 3 + digits : 2, '0');
	return `${minutes}:${text}`;
}
