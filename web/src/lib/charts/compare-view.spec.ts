// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from 'vitest';
import type { CompareData, LapTrace } from '#lib/api/index.ts';
import {
	carIndex,
	firstIndex,
	gapText,
	gearAt,
	headingAt,
	indexAtTime,
	lastIndex,
	pedalAt,
	positionAt,
	rebaseDeltas,
	sectionOf,
	sectionTime,
	steeringAt,
	valueAt
} from './compare-view.ts';
import { formatDeltaS, formatGear, formatKm } from './compare-panels.ts';

function lap(t: (number | null)[], deltaS: (number | null)[] = t.map(() => 0)): LapTrace {
	return {
		lapId: 'l',
		sessionId: 's',
		number: 1,
		lapTimeMs: null,
		valid: true,
		// a straight line north, 1 m per grid point
		channels: { t, pos_x: t.map(() => 0), pos_y: t.map((_, i) => i) },
		deltaS
	};
}

// grid of 1 m; the reference covers a metre per second, lap 1 is twice as fast
const data: CompareData = {
	stepM: 1,
	distM: [0, 1, 2, 3, 4, 5],
	units: {},
	laps: [lap([0, 1, 2, 3, 4, 5]), lap([0, 0.5, 1, 1.5, 2, 2.5], [0, -0.5, -1, -1.5, -2, -2.5])]
};

describe('valueAt', () => {
	it('interpolates and respects gaps', () => {
		expect(valueAt([0, 10, 20], 1.5)).toBe(15);
		expect(valueAt([0, null, 20], 1)).toBeNull();
		expect(valueAt([0, 10], 5)).toBeNull();
	});
});

describe('indexAtTime', () => {
	it('finds the fractional index of a time', () => {
		expect(indexAtTime([0, 1, 2, 3], 1.25)).toBeCloseTo(1.25);
		expect(indexAtTime([null, null, 5, 6, 7], 6.5)).toBeCloseTo(3.5);
		expect(indexAtTime([0, 1, 2], 3)).toBeNull();
		expect(indexAtTime([5, 6], 4)).toBeNull();
	});
});

describe('carIndex', () => {
	it('places cars by distance or by time', () => {
		expect(carIndex(data, 1, 2, 'distance')).toBe(2);
		// after 2 s the faster lap is at 4 m
		expect(carIndex(data, 1, 2, 'time')).toBeCloseTo(4);
		// the faster lap has finished: not on track any more
		expect(carIndex(data, 1, 5, 'time')).toBeNull();
	});
});

describe('positions', () => {
	it('knows where a car is and where it heads', () => {
		expect(positionAt(data.laps[0], 2.5)).toEqual({ x: 0, y: 2.5 });
		expect(headingAt(data.laps[0], 2, 1, 1)).toBeCloseTo(Math.PI / 2); // north
		expect([firstIndex(data), lastIndex(data)]).toEqual([0, 5]);
	});
});

describe('gapText', () => {
	it('shows the time gap by distance and the distance gap by time', () => {
		expect(gapText(data, 0, 2, 'time')).toBe('reference');
		expect(gapText(data, 1, 2, 'distance')).toBe('−1.000 s');
		expect(gapText(data, 1, 2, 'time')).toBe('2 m ahead');
	});
});

describe('chart labels', () => {
	it('formats distance, gear and delta', () => {
		expect(formatKm(1200, 200)).toBe('1.2 km');
		expect(formatKm(1150, 50)).toBe('1.15 km');
		expect(formatKm(2000, 1000)).toBe('2 km');
		expect([formatGear(-1), formatGear(0), formatGear(3)]).toEqual(['R', 'N', '3']);
		expect(formatDeltaS(-0.2804)).toBe('−0.280 s');
	});
});

describe('sections', () => {
	it('turns a distance range into grid indices within the lap', () => {
		expect(sectionOf(data, null)).toEqual({ start: 0, end: 5 });
		expect(sectionOf(data, { min: 1.4, max: 3.6 })).toEqual({ start: 2, end: 3 });
		expect(sectionOf(data, { min: -10, max: 99 })).toEqual({ start: 0, end: 5 });
	});

	it('counts the diff from the section start', () => {
		const rebased = rebaseDeltas(data, 2);
		expect(rebased.laps[1].deltaS).toEqual([1, 0.5, 0, -0.5, -1, -1.5]);
		expect(data.laps[1].deltaS[2]).toBe(-1); // the original stays
	});

	it('times each lap through the section', () => {
		const s = { start: 1, end: 4 };
		expect(sectionTime(data.laps[0], s)).toBe(3);
		expect(sectionTime(data.laps[1], s)).toBe(1.5);
	});
});

describe('steeringAt', () => {
	it('reads the wheel angle in degrees, else the input in percent', () => {
		const angle = { ...lap([0, 1]), channels: { steer: [-Math.PI / 2, Math.PI / 4] } };
		expect(steeringAt(angle, 0)).toEqual({ text: '−90°', turn: -90 });
		expect(steeringAt(angle, 1)?.text).toBe('45°');
		const input = { ...lap([0, 1]), channels: { steer_norm: [0.5, null] } };
		expect(steeringAt(input, 0)).toEqual({ text: '50%', turn: 90 });
		expect(steeringAt(input, 1)).toBeNull();
	});
});

describe('gearAt', () => {
	it('takes the nearest grid point', () => {
		const l = { ...lap([0, 1, 2]), channels: { gear: [2, 3, null] } };
		expect(gearAt(l, 0.4)).toBe(2);
		expect(gearAt(l, 0.6)).toBe(3);
		expect(gearAt(l, 2)).toBeNull();
		expect(gearAt(lap([0]), 0)).toBeNull();
	});
});

describe('pedalAt', () => {
	it('is the pedal in whole percent, within 0–100', () => {
		const l = {
			...lap([0, 1, 2]),
			channels: { throttle: [0.5, 1.02, null], brake: [0.123, 0, 0] }
		};
		expect(pedalAt(l, 'throttle', 0)).toBe(50);
		expect(pedalAt(l, 'throttle', 1)).toBe(100);
		expect(pedalAt(l, 'throttle', 2)).toBeNull();
		expect(pedalAt(l, 'brake', 0)).toBe(12);
		expect(pedalAt(l, 'clutch', 0)).toBeNull();
	});
});
