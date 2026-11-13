// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from 'vitest';
import type { LapConditions, LapSummary } from './api/index.ts';
import {
	anyFlat,
	cellText,
	compoundColor,
	compoundDots,
	fuelUsedText,
	tireSummary,
	visibleColumns,
	wheelDetails
} from './lap-conditions.ts';

const none: LapConditions = {
	airTempC: null,
	trackTempC: null,
	humidity: null,
	weather: null,
	fuelStart: null,
	fuelUsed: null,
	fuelUnit: null,
	tyreTempC: null,
	tyreWear: null
};

function lap(conditions: Partial<LapConditions> | null): LapSummary {
	return {
		id: 'l',
		number: 1,
		lapTimeMs: 90_000,
		sectorTimesMs: [],
		valid: true,
		invalidReason: null,
		outLap: false,
		inLap: false,
		incomplete: false,
		flashback: false,
		conditions: conditions && { ...none, ...conditions }
	};
}

describe('visibleColumns', () => {
	it('shows only what some lap has', () => {
		const keys = visibleColumns([
			lap({ fuelStart: 10, fuelUnit: 'kg' }),
			lap({ airTempC: 21 }),
			lap(null)
		]).map((c) => c.key);
		expect(keys).toEqual(['airTemp', 'fuel']);
		expect(visibleColumns([lap(null)])).toEqual([]);
	});
});

describe('cellText', () => {
	const c = {
		...none,
		airTempC: 20.66,
		humidity: 0.394,
		fuelStart: 85.58,
		fuelUsed: 3.014,
		fuelUnit: 'l',
		tyreTempC: { fl: 60, fr: 80, rl: 60, rr: 55 },
		tyreWear: { fl: 0.01, fr: 0.02, rl: 0.015, rr: 0.011 }
	};

	it('formats each column', () => {
		expect(cellText(c, 'airTemp')).toBe('20.7 °C');
		expect(cellText(c, 'trackTemp')).toBe('');
		expect(cellText(c, 'humidity')).toBe('39 %');
		expect(cellText(c, 'fuel')).toBe('85.6 l');
		expect(cellText(c, 'tires')).toBe('64 °C 1.4 %');
		expect(cellText(null, 'weather')).toBe('');
	});

	it('has the fuel used for the tooltip', () => {
		expect(fuelUsedText(c)).toBe('Used 3.01 l in this lap');
		expect(fuelUsedText({ ...c, fuelUsed: null })).toBeUndefined();
	});

	it('knows about flat tyres', () => {
		expect(anyFlat(c)).toBe(false);
		expect(anyFlat({ ...c, tyreFlat: { fl: false, fr: true, rl: false, rr: false } })).toBe(true);
	});
});

describe('tires', () => {
	const c: LapConditions = {
		...none,
		tyreTempC: { fl: 60, fr: 80, rl: 60, rr: 56 },
		tyreWear: { fl: 0.05, fr: 0.02, rl: 0.015, rr: 0.011 },
		tyreCompound: {
			fl: 'Soft',
			fr: 'Soft',
			rl: 'Soft',
			rr: 'Soft',
			detail: 'C5',
			ageLaps: 5,
			before: null
		}
	};

	it('sums up compound, temperature and wear, leaving out what is missing', () => {
		expect(cellText(c, 'tires')).toBe('Soft 64 °C 2.4 %');
		expect(cellText({ ...c, tyreCompound: null }, 'tires')).toBe('64 °C 2.4 %');
		expect(cellText({ ...none, tyreTempC: c.tyreTempC }, 'tires')).toBe('64 °C');
		expect(visibleColumns([lap({ tyreWear: c.tyreWear })]).map((col) => col.key)).toEqual([
			'tires'
		]);
	});

	it('says Mixed for different compounds or a change in the lap', () => {
		const t = c.tyreCompound!;
		expect(tireSummary({ ...c, tyreCompound: { ...t, rr: 'Medium' } }).compound).toBe('Mixed');
		const before = { fl: 'Hard', fr: 'Soft', rl: 'Soft', rr: 'Soft' };
		expect(tireSummary({ ...c, tyreCompound: { ...t, before } }).compound).toBe('Mixed');
	});

	it('details every tire', () => {
		const [fl, , rl, rr] = wheelDetails({
			...c,
			tyreCompound: { ...c.tyreCompound!, rl: 'Medium', rr: 'Hard', detail: null, ageLaps: 1 },
			tyreFlat: { fl: false, fr: false, rl: true, rr: false }
		});
		expect(fl).toMatchObject({ label: 'Front left', compound: 'Soft – 1 lap', temp: '60 °C' });
		expect(rl).toMatchObject({ compound: 'Medium – 1 lap', wear: '1.5 %', flat: true });
		expect(rr).toMatchObject({ compound: 'Hard – 1 lap', compoundName: 'Hard' });
		expect(wheelDetails(c)[0].compound).toBe('Soft (C5) – 5 laps');
		expect(wheelDetails({ ...none, tyreWear: c.tyreWear })[1]).toMatchObject({
			compound: null,
			temp: null,
			wear: '2.0 %'
		});
	});

	it('colours the usual compounds', () => {
		// F1: soft red, hard white
		expect(compoundColor('Super soft', 'f1_25').bg).toBe('bg-red-500');
		expect(compoundColor('Hard', 'f1_25').bg).toBe('bg-zinc-100');
		expect(compoundColor('Intermediate', 'f1_25').text).toBe('text-green-500');
		// LMU: soft white, hard red
		expect(compoundColor('Soft', 'lmu').bg).toBe('bg-zinc-100');
		expect(compoundColor('Hard', 'lmu').bg).toBe('bg-red-500');
		expect(compoundColor('Wet', 'lmu').bg).toBe('bg-blue-500');
		// iRacing: only wet has a colour
		expect(compoundColor('Hard', 'iracing').bg).toBe('bg-zinc-500');
		expect(compoundColor('Wet', 'iracing').bg).toBe('bg-blue-500');
		expect(compoundColor('Mixed', 'lmu').bg).toBe('bg-zinc-500');
	});
});

describe('compound dots', () => {
	type W = [string, string, string, string];
	const set = (after: W, before: W | null = null): LapConditions => ({
		...none,
		tyreCompound: {
			fl: after[0],
			fr: after[1],
			rl: after[2],
			rr: after[3],
			detail: null,
			ageLaps: null,
			before: before && { fl: before[0], fr: before[1], rl: before[2], rr: before[3] }
		}
	});
	/** each dot as "before|after" names, or the name alone */
	const dots = (c: LapConditions) =>
		compoundDots(c, 'lmu').map((d) =>
			d.before ? `${d.before.name}|${d.after.name}` : d.after.name
		);

	it('has one dot if all four are the same', () => {
		expect(dots(set(['Soft', 'Soft', 'Soft', 'Soft']))).toEqual(['Soft']);
		expect(compoundDots(set(['Soft', 'Soft', 'Soft', 'Soft']), 'lmu')[0].label).toBe('All tires');
	});

	it('has two dots if front and rear differ', () => {
		expect(dots(set(['Soft', 'Soft', 'Hard', 'Hard']))).toEqual(['Soft', 'Hard']);
	});

	it('has four dots as soon as an axle is uneven', () => {
		expect(dots(set(['Hard', 'Medium', 'Wet', 'Hard']))).toEqual(['Hard', 'Medium', 'Wet', 'Hard']);
		expect(dots(set(['Soft', 'Soft', 'Hard', 'Medium']))).toHaveLength(4);
	});

	it('splits the dots of tires changed in the lap', () => {
		// all four soft to hard: one dot, split
		expect(dots(set(['Hard', 'Hard', 'Hard', 'Hard'], ['Soft', 'Soft', 'Soft', 'Soft']))).toEqual([
			'Soft|Hard'
		]);
		// soft front, hard rear; only the front changed to hard: two dots
		expect(dots(set(['Hard', 'Hard', 'Hard', 'Hard'], ['Soft', 'Soft', 'Hard', 'Hard']))).toEqual([
			'Soft|Hard',
			'Hard'
		]);
		// four softs, one changed to hard: four dots, one split
		expect(dots(set(['Hard', 'Soft', 'Soft', 'Soft'], ['Soft', 'Soft', 'Soft', 'Soft']))).toEqual([
			'Soft|Hard',
			'Soft',
			'Soft',
			'Soft'
		]);
	});

	it("colours both halves in the game's colours", () => {
		const [d] = compoundDots(
			set(['Hard', 'Hard', 'Hard', 'Hard'], ['Soft', 'Soft', 'Soft', 'Soft']),
			'lmu'
		);
		expect([d.before?.bg, d.after.bg]).toEqual(['bg-zinc-100', 'bg-red-500']);
	});
});

describe('tire details after a change', () => {
	it('shows before and after per tire', () => {
		const c: LapConditions = {
			...none,
			tyreCompound: {
				fl: 'Hard',
				fr: 'Soft',
				rl: 'Soft',
				rr: 'Soft',
				detail: null,
				ageLaps: null,
				before: { fl: 'Soft', fr: 'Soft', rl: 'Soft', rr: 'Soft' }
			}
		};
		const [fl, fr] = wheelDetails(c);
		expect(fl.compound).toBe('Soft → Hard');
		expect(fr.compound).toBe('Soft');
	});
});
