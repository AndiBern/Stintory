// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from 'vitest';
import type { CompareData, Corner, CornerLapStats } from '#lib/api/index.ts';
import { byTimeLost, cornerNotes, cornerRange, cornerRows, formatSeconds } from './corners.ts';

function stats(s: Partial<CornerLapStats> = {}): CornerLapStats {
	return {
		timeS: 5,
		timeLostS: 0,
		entryLostS: 0,
		exitLostS: 0,
		topSpeedMps: 50,
		liftM: 297,
		brakeM: 300,
		brakeMax: 0.9,
		brakeReleaseM: 380,
		minSpeedMps: 20,
		minSpeedM: 400,
		minSpeedGear: 2,
		throttleM: 398,
		fullThrottleM: 430,
		exitSpeedMps: 30,
		flatOut: false,
		...s
	};
}

function corner(number: number, start: number, end: number): Corner {
	return {
		number,
		direction: 'right',
		startM: start + 50,
		apexM: start + 100,
		endM: start + 150,
		segmentStartM: start,
		segmentEndM: end,
		minRadiusM: 30
	};
}

describe('cornerNotes', () => {
	it('names what the lap does differently, in driving order', () => {
		const lap = stats({
			brakeM: 280,
			minSpeedMps: 19,
			minSpeedGear: 1,
			throttleM: 410,
			fullThrottleM: 445,
			exitSpeedMps: 29
		});
		expect(cornerNotes(lap, stats())).toEqual([
			'brakes 20 m earlier',
			'4 km/h slower at the apex',
			'gear 1 instead of 2',
			'on the throttle 12 m later',
			'full throttle 15 m later',
			'4 km/h slower on exit'
		]);
	});

	it('ignores differences that are noise', () => {
		const lap = stats({ brakeM: 302, minSpeedMps: 20.1, throttleM: 395, exitSpeedMps: 30.2 });
		expect(cornerNotes(lap, stats())).toEqual([]);
	});

	it('handles corners without braking', () => {
		const flat = stats({ brakeM: null, liftM: null, flatOut: true, throttleM: null });
		const lifting = stats({ brakeM: null, liftM: 290, throttleM: null });
		expect(cornerNotes(lifting, flat)).toEqual(['lifts, the reference is flat out']);
		expect(cornerNotes(flat, lifting)).toEqual(['flat out, the reference lifts']);
		expect(cornerNotes(stats(), lifting)[0]).toBe('brakes, the reference doesn’t');
		expect(cornerNotes(lifting, stats())[0]).toBe('doesn’t brake');
		expect(cornerNotes(stats({ brakeM: null, liftM: 270, throttleM: null }), lifting)).toEqual([
			'lifts 20 m earlier'
		]);
	});
});

describe('corner rows', () => {
	const data: CompareData = {
		stepM: 1,
		distM: [],
		units: {},
		laps: [],
		corners: {
			corners: [corner(1, 100, 600), corner(2, 600, 1200), corner(3, 1200, 100)],
			laps: [
				{ lapId: 'a', corners: [stats(), stats(), stats()] },
				{
					lapId: 'b',
					corners: [
						stats({ timeLostS: 0.1 }),
						stats({ timeLostS: null }),
						stats({ timeLostS: 0.3, brakeM: 290 })
					]
				}
			]
		}
	};

	it('compares a lap with the reference', () => {
		const rows = cornerRows(data, 1);
		expect(rows.map((r) => r.corner.number)).toEqual([1, 2, 3]);
		expect(rows[2].notes).toEqual(['brakes 10 m earlier']);
		expect(cornerRows(data, 0)[2].notes).toEqual([]);
		expect(cornerRows({ ...data, corners: null }, 1)).toEqual([]);
	});

	it('sorts by time lost, unknown last', () => {
		expect(byTimeLost(cornerRows(data, 1)).map((r) => r.corner.number)).toEqual([3, 1, 2]);
	});

	it('shows the last corner up to the lap end', () => {
		expect(cornerRange(corner(1, 100, 600), 2000)).toEqual({ min: 100, max: 600 });
		expect(cornerRange(corner(3, 1200, 100), 2000)).toEqual({ min: 1200, max: 2000 });
	});

	it('formats seconds with a sign', () => {
		expect(formatSeconds(0.1234)).toBe('+0.123');
		expect(formatSeconds(-0.05)).toBe('−0.050');
		expect(formatSeconds(null)).toBe('–');
	});
});
