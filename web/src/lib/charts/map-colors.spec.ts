// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from 'vitest';
import type { CompareData, LapTrace } from '#lib/api/index.ts';
import { colorOptions, gainRate, lineColoring, rampColors } from './map-colors.ts';

function lap(channels: Record<string, (number | null)[]>, deltaS = [0, 0, 0, 0]): LapTrace {
	return { lapId: 'l', sessionId: 's', number: 1, lapTimeMs: null, valid: true, channels, deltaS };
}

function data(...laps: LapTrace[]): CompareData {
	return { stepM: 1, distM: [0, 1, 2, 3], units: {}, laps };
}

describe('colorOptions', () => {
	it('offers only values at least one lap has', () => {
		const d = data(lap({ speed: [10, 20, 30, 40] }), lap({ throttle: [0, 0.5, 1, null] }));
		expect(colorOptions(d).map((o) => o.key)).toEqual(['lap', 'speed', 'throttle', 'gain']);
	});

	it('has no gain or loss for a single lap, nor channels without values', () => {
		const d = data(lap({ brake: [null, null, null, null] }));
		expect(colorOptions(d).map((o) => o.key)).toEqual(['lap']);
	});
});

describe('lineColoring', () => {
	it('is null for the lap colour', () => {
		expect(lineColoring(data(lap({})), 'lap')).toBeNull();
	});

	it('puts throttle from black (0) to green (100 %) and names laps without it', () => {
		const d = data(lap({ throttle: [0, 0.5, 1, null] }), lap({ speed: [1, 2, 3, 4] }));
		const c = lineColoring(d, 'throttle')!;
		const bins = c.bins[0]!;
		expect(c.palette[bins[0]]).toBe('#000000');
		expect(c.palette[bins[2]]).toBe('#3dff6e');
		expect(bins[3]).toBe(-1);
		expect(c.bins[1]).toBeNull();
		expect(c.missing).toEqual([1]);
		expect(c.legend).toMatchObject({ kind: 'ramp', min: '0 %', max: '100 %' });
	});

	it('scales speed over all laps in km/h, rounded to 10', () => {
		const d = data(lap({ speed: [20, 30, 40, 50] }), lap({ speed: [25, 35, 45, 55] }));
		expect(lineColoring(d, 'speed')!.legend).toMatchObject({ min: '70 km/h', max: '200 km/h' });
	});

	it('gives each gear driven its own colour', () => {
		const c = lineColoring(data(lap({ gear: [2, 3, 3, 5] })), 'gear')!;
		expect(c.palette).toHaveLength(4);
		expect(Array.from(c.bins[0]!)).toEqual([0, 1, 1, 3]);
		expect(c.legend.kind === 'steps' && c.legend.items.map((i) => i.label)).toEqual([
			'2',
			'3',
			'4',
			'5'
		]);
	});

	it('shows ABS as off or on', () => {
		const c = lineColoring(data(lap({ abs_active: [0, 1, 0.2, null] })), 'abs')!;
		expect(Array.from(c.bins[0]!)).toEqual([0, 1, 0, -1]);
	});
});

describe('gainRate', () => {
	it('is the diff gained per 100 m, positive when losing', () => {
		// 0.01 s lost per metre → 1 s per 100 m
		const delta = Array.from({ length: 30 }, (_, i) => i * 0.01);
		const r = gainRate(delta, 1);
		expect(r[0]).toBeNull();
		expect(r[15]).toBeCloseTo(1);
		expect(r[29]).toBeNull();
	});
});

describe('rampColors', () => {
	it('runs from the first stop to the last', () => {
		expect(rampColors(['#000000', '#ffffff'], 3)).toEqual(['#000000', '#808080', '#ffffff']);
	});
});
