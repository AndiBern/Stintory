// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from 'vitest';
import type { LapSummary } from '#lib/api/index.ts';
import { formatTick, isClean, lapTicks, timeScale } from './lap-times.ts';

function lap(number: number, lapTimeMs: number | null, over: Partial<LapSummary> = {}): LapSummary {
	return {
		id: `l${number}`,
		number,
		lapTimeMs,
		sectorTimesMs: [],
		valid: true,
		invalidReason: null,
		outLap: false,
		inLap: false,
		incomplete: false,
		flashback: false,
		...over
	};
}

describe('timeScale', () => {
	it('is null without timed laps', () => {
		expect(timeScale([lap(1, null)])).toBeNull();
	});

	it('fits the clean laps with some room and clean ticks', () => {
		const s = timeScale([lap(1, 95_000), lap(2, 96_000), lap(3, 97_000)])!;
		expect(s.min).toBeCloseTo(94_750);
		expect(s.max).toBeCloseTo(97_250);
		expect(s.step).toBe(500);
		expect(s.ticks).toEqual([95_000, 95_500, 96_000, 96_500, 97_000]);
	});

	it('leaves a slow out-lap off the scale but keeps a close invalid lap', () => {
		const s = timeScale([
			lap(1, 130_000, { outLap: true }),
			lap(2, 95_000),
			lap(3, 99_000, { valid: false }),
			lap(4, 96_000)
		])!;
		expect(s.max).toBeGreaterThan(99_000);
		expect(s.max).toBeLessThan(130_000);
	});

	it('leaves a partial out-lap that is far faster off the scale', () => {
		// F1 25 time trial: the out-lap only runs from the garage to the line
		const s = timeScale([
			lap(0, 24_076, { outLap: true }),
			lap(1, 77_429, { valid: false }),
			lap(2, 75_689),
			lap(3, 75_165),
			lap(4, 86_986, { valid: false }),
			lap(5, 75_909)
		])!;
		expect(s.min).toBeGreaterThan(70_000);
		expect(s.max).toBeGreaterThan(77_429);
		expect(s.max).toBeLessThan(86_986);
	});

	it('uses all timed laps when none is clean', () => {
		const s = timeScale([lap(1, 95_000, { valid: false }), lap(2, 96_000, { valid: false })])!;
		expect(s.min).toBeLessThan(95_000);
		expect(s.max).toBeGreaterThan(96_000);
	});
});

describe('lapTicks', () => {
	it('labels every lap while they fit, then thins out', () => {
		expect(lapTicks(1, 5, 10)).toEqual([1, 2, 3, 4, 5]);
		expect(lapTicks(1, 40, 10)).toEqual([1, 5, 10, 15, 20, 25, 30, 35, 40]);
	});
});

describe('formatTick', () => {
	it('shows decimals only when the step needs them', () => {
		expect(formatTick(95_000, 1000)).toBe('1:35');
		expect(formatTick(65_000, 1000)).toBe('1:05');
		expect(formatTick(95_500, 500)).toBe('1:35.5');
		expect(formatTick(95_250, 250)).toBe('1:35.25');
	});
});

describe('isClean', () => {
	it('excludes invalid, out-, in- and incomplete laps', () => {
		expect(isClean(lap(1, 95_000))).toBe(true);
		expect(isClean(lap(1, 95_000, { valid: false }))).toBe(false);
		expect(isClean(lap(1, 95_000, { inLap: true }))).toBe(false);
		expect(isClean(lap(1, null))).toBe(false);
	});
});
