// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from 'vitest';
import type { LapConditions, LapSummary } from './api/index.ts';
import {
	anyFlat,
	cellText,
	compoundColor,
	compoundDetails,
	fuelUsedText,
	visibleColumns
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
		expect(cellText(c, 'tyreTemp')).toBe('64 °C');
		expect(cellText(c, 'tyreWear')).toBe('1.4 %');
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

describe('tyre compound', () => {
	const c = {
		...none,
		tyreCompound: { name: 'Hard', detail: 'C3', changedFrom: 'Medium', ageLaps: 0 }
	};

	it('shows the compound and a change in the lap', () => {
		expect(cellText(c, 'tyres')).toBe('Medium → Hard');
		expect(compoundDetails(c)).toBe('C3 · 0 laps old · changed from Medium in this lap');
		expect(visibleColumns([lap(c)]).map((col) => col.key)).toEqual(['tyres']);
		expect(
			compoundDetails({
				...c,
				tyreCompound: { name: 'Wet', detail: null, changedFrom: null, ageLaps: null }
			})
		).toBeUndefined();
	});

	it('colours the usual compounds', () => {
		expect(compoundColor('Super soft')).toBe('bg-red-500');
		expect(compoundColor('Intermediate')).toBe('bg-green-500');
		expect(compoundColor('All purpose')).toBe('bg-zinc-500');
	});
});
