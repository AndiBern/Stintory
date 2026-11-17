// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * Colouring the track map's lines by a telemetry value instead of the lap
 * colour. Every lap uses the same scale, so frames side by side compare.
 * Values are put into bins (an index into the palette) once per lap, so the
 * map can draw runs of the same colour in one stroke.
 */

import type { CompareData } from '#lib/api/index.ts';
import { formatGear } from './compare-panels.ts';

export type ColorBy =
	'lap' | 'speed' | 'throttle' | 'brake' | 'gear' | 'steer' | 'gain' | 'abs' | 'tc';

export interface ColorOption {
	key: ColorBy;
	label: string;
}

/** A colour bar from left to right, with the values at its ends. */
export interface RampLegend {
	kind: 'ramp';
	title: string;
	/** CSS gradient of the palette */
	gradient: string;
	min: string;
	max: string;
	/** what the ends mean, e.g. gaining / losing */
	minNote?: string;
	maxNote?: string;
}

/** Separate swatches, e.g. one per gear. */
export interface StepsLegend {
	kind: 'steps';
	title: string;
	items: { label: string; color: string }[];
}

export interface LineColoring {
	key: ColorBy;
	palette: string[];
	/** per lap: palette index per grid point (-1 = no value), null = lap has no such data */
	bins: (Int16Array | null)[];
	legend: RampLegend | StepsLegend;
	/** laps without the value (drawn grey, dashed) */
	missing: number[];
}

/** Bins of a continuous scale. */
const LEVELS = 64;
const RAD_TO_DEG = 180 / Math.PI;
/** The grey midpoint of the diverging scales and "off". */
const NEUTRAL = '#71717a';

/** Full throttle and full brake, on the map and in the pedal readouts. */
export const PEDAL_COLORS = { throttle: '#3dff6e', brake: '#ff3434' };

const RAMPS = {
	speed: ['#000000', '#104281', '#3987e5', '#cde2fb'],
	throttle: ['#000000', PEDAL_COLORS.throttle],
	brake: ['#000000', PEDAL_COLORS.brake],
	gear: ['#3d3480', '#9085e9', '#ece9ff'],
	// left / gaining … right / losing
	diverging: ['#3987e5', NEUTRAL, '#e5484d']
};
const ON = '#fab219';

type Series = (number | null)[];

function hasValues(values: Series | undefined): boolean {
	return !!values && values.some((v) => v != null);
}

function anyLap(d: CompareData, channel: string): boolean {
	return d.laps.some((l) => hasValues(l.channels[channel]));
}

/** The steering channel the laps have: the angle, or the input for F1. */
function steerChannel(d: CompareData): 'steer' | 'steer_norm' | null {
	if (anyLap(d, 'steer')) return 'steer';
	if (anyLap(d, 'steer_norm')) return 'steer_norm';
	return null;
}

/** The colourings the laps have data for (at least one lap); the lap colour first. */
export function colorOptions(d: CompareData): ColorOption[] {
	const list: ColorOption[] = [{ key: 'lap', label: 'Lap colour' }];
	if (anyLap(d, 'speed')) list.push({ key: 'speed', label: 'Speed' });
	if (anyLap(d, 'throttle')) list.push({ key: 'throttle', label: 'Throttle' });
	if (anyLap(d, 'brake')) list.push({ key: 'brake', label: 'Brake' });
	if (anyLap(d, 'gear')) list.push({ key: 'gear', label: 'Gear' });
	if (steerChannel(d)) list.push({ key: 'steer', label: 'Steering' });
	if (d.laps.length > 1) list.push({ key: 'gain', label: 'Gain / loss to ref' });
	if (anyLap(d, 'abs_active')) list.push({ key: 'abs', label: 'ABS active' });
	if (anyLap(d, 'tc_active')) list.push({ key: 'tc', label: 'TC active' });
	return list;
}

function hex(c: string): [number, number, number] {
	const n = parseInt(c.slice(1), 16);
	return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

/** `n` colours evenly along the stops (interpolated in sRGB). */
export function rampColors(stops: string[], n: number): string[] {
	const rgb = stops.map(hex);
	return Array.from({ length: n }, (_, i) => {
		const f = n === 1 ? 0 : (i / (n - 1)) * (stops.length - 1);
		const k = Math.min(stops.length - 2, Math.floor(f));
		const t = f - k;
		const [a, b] = [rgb[k], rgb[k + 1]];
		const c = a.map((v, j) => Math.round(v + (b[j] - v) * t));
		return `#${c.map((v) => v.toString(16).padStart(2, '0')).join('')}`;
	});
}

function gradient(stops: string[]): string {
	return `linear-gradient(to right, ${stops.join(', ')})`;
}

/** The value at fraction `q` (0–1) of the sorted values. */
function quantile(values: number[], q: number): number {
	if (values.length === 0) return 0;
	const s = [...values].sort((a, b) => a - b);
	return s[Math.min(s.length - 1, Math.max(0, Math.round(q * (s.length - 1))))];
}

function finite(series: (Series | null)[]): number[] {
	return series.flatMap((s) => (s ?? []).filter((v): v is number => v != null && isFinite(v)));
}

/** Bins of a continuous value between `lo` and `hi`. */
function continuousBins(values: Series, lo: number, hi: number): Int16Array {
	const span = hi - lo || 1;
	const out = new Int16Array(values.length);
	values.forEach((v, i) => {
		out[i] = v == null ? -1 : Math.round(Math.min(1, Math.max(0, (v - lo) / span)) * (LEVELS - 1));
	});
	return out;
}

/**
 * Time won or lost against the reference (s per 100 m, positive = losing),
 * from the diff over ±10 m around each point.
 */
export function gainRate(deltaS: Series, stepM: number): Series {
	const w = Math.max(1, Math.round(10 / stepM));
	return deltaS.map((_, i) => {
		const a = deltaS[i - w];
		const b = deltaS[i + w];
		return a == null || b == null ? null : ((b - a) / (2 * w * stepM)) * 100;
	});
}

/** The colouring for `key`; null for the lap colour. */
export function lineColoring(d: CompareData, key: ColorBy): LineColoring | null {
	const perLap = (channel: string, scale = (v: number) => v): (Series | null)[] =>
		d.laps.map((l) => {
			const s = l.channels[channel];
			return hasValues(s) ? s!.map((v) => (v == null ? null : scale(v))) : null;
		});
	const missingOf = (series: (Series | null)[]) =>
		series.flatMap((s, k) => (s === null ? [k] : []));

	const continuous = (
		series: (Series | null)[],
		stops: string[],
		lo: number,
		hi: number,
		legend: Omit<RampLegend, 'kind' | 'gradient'>
	): LineColoring => ({
		key,
		palette: rampColors(stops, LEVELS),
		bins: series.map((s) => (s ? continuousBins(s, lo, hi) : null)),
		legend: { kind: 'ramp', gradient: gradient(stops), ...legend },
		missing: missingOf(series)
	});

	switch (key) {
		case 'lap':
			return null;
		case 'speed': {
			const series = perLap('speed', (v) => v * 3.6);
			const all = finite(series);
			const lo = Math.floor(Math.min(...all) / 10) * 10;
			const hi = Math.max(lo + 10, Math.ceil(Math.max(...all) / 10) * 10);
			return continuous(series, RAMPS.speed, lo, hi, {
				title: 'Speed',
				min: `${lo} km/h`,
				max: `${hi} km/h`
			});
		}
		case 'throttle':
		case 'brake': {
			const series = perLap(key, (v) => v * 100);
			return continuous(series, RAMPS[key], 0, 100, {
				title: key === 'throttle' ? 'Throttle' : 'Brake',
				min: '0 %',
				max: '100 %'
			});
		}
		case 'steer': {
			const channel = steerChannel(d) ?? 'steer';
			const degrees = channel === 'steer';
			const series = perLap(channel, (v) => (degrees ? v * RAD_TO_DEG : v * 100));
			// symmetric, so straight ahead is the grey middle; spikes don't stretch it
			const all = finite(series).map(Math.abs);
			const m = Math.max(5, Math.ceil(quantile(all, 0.98)));
			const unit = degrees ? '°' : ' %';
			return continuous(series, RAMPS.diverging, -m, m, {
				title: 'Steering',
				min: `${m}${unit}`,
				max: `${m}${unit}`,
				minNote: 'left',
				maxNote: 'right'
			});
		}
		case 'gain': {
			// the reference is the baseline: no gain or loss against itself
			const series: (Series | null)[] = d.laps.map((l, k) =>
				k === 0 ? d.distM.map(() => 0) : gainRate(l.deltaS, d.stepM)
			);
			const all = finite(series.slice(1)).map(Math.abs);
			const m = Math.max(0.005, quantile(all, 0.95));
			const digits = m >= 0.1 ? 2 : 3;
			return continuous(series, RAMPS.diverging, -m, m, {
				title: 'Gain / loss to ref per 100 m',
				min: `−${m.toFixed(digits)} s`,
				max: `+${m.toFixed(digits)} s`,
				minNote: 'gaining',
				maxNote: 'losing'
			});
		}
		case 'gear': {
			const series = perLap('gear', (v) => Math.round(v));
			const gears = finite(series);
			const lo = Math.min(...gears);
			const hi = Math.max(...gears);
			const palette = rampColors(RAMPS.gear, hi - lo + 1);
			return {
				key,
				palette,
				bins: series.map((s) => (s ? Int16Array.from(s, (v) => (v == null ? -1 : v - lo)) : null)),
				legend: {
					kind: 'steps',
					title: 'Gear',
					items: palette.map((color, i) => ({ label: formatGear(lo + i), color }))
				},
				missing: missingOf(series)
			};
		}
		case 'abs':
		case 'tc': {
			const series = perLap(key === 'abs' ? 'abs_active' : 'tc_active');
			return {
				key,
				palette: [NEUTRAL, ON],
				bins: series.map((s) =>
					s ? Int16Array.from(s, (v) => (v == null ? -1 : v >= 0.5 ? 1 : 0)) : null
				),
				legend: {
					kind: 'steps',
					title: key === 'abs' ? 'ABS active' : 'TC active',
					items: [
						{ label: 'off', color: NEUTRAL },
						{ label: 'on', color: ON }
					]
				},
				missing: missingOf(series)
			};
		}
	}
}
