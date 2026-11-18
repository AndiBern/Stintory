// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

/** The charts of the comparison page: what each shows and how values read. */

import type { CompareData } from '#lib/api/index.ts';

/** Shared zoom of the charts (metres); null = the whole lap. */
export type XRange = { min: number; max: number } | null;

export interface PanelLine {
	channel: string;
	/** stored value (SI) → shown value */
	scale: (v: number) => number;
	dash?: boolean;
	/** for panels with several lines per lap */
	name?: string;
}

export interface Panel {
	key: string;
	title: string;
	unit: string;
	height: number;
	lines: PanelLine[];
	/** time difference to the reference instead of channels */
	delta?: boolean;
	stepped?: boolean;
	/** fixed value range */
	range?: [number, number];
	/** whole values only (gear, modes, flags): one tick per value, labelled by `format` */
	discrete?: boolean;
	/** a value as the tooltip shows it */
	format: (v: number) => string;
}

const RAD_TO_DEG = 180 / Math.PI;
const fixed = (digits: number, unit: string) => (v: number) =>
	`${v.toFixed(digits)}${unit ? ` ${unit}` : ''}`;

/** Distance axis label with as many decimals as the tick spacing needs. */
export function formatKm(m: number, stepM: number): string {
	const digits = stepM >= 1000 ? 0 : stepM >= 100 ? 1 : stepM >= 10 ? 2 : 3;
	return `${(m / 1000).toFixed(digits)} km`;
}

export function formatGear(v: number): string {
	const g = Math.round(v);
	return g < 0 ? 'R' : g === 0 ? 'N' : String(g);
}

/** Seconds with sign, e.g. `+0.231 s`. */
export function formatDeltaS(v: number): string {
	return `${v >= 0 ? '+' : '−'}${Math.abs(v).toFixed(3)} s`;
}

function has(d: CompareData, channel: string): boolean {
	return d.laps.some((l) => channel in l.channels);
}

/**
 * True if a lap's channel is non-zero somewhere. Games send 0 for what the
 * car doesn't have (F1 25: DRS on 2026 cars, ERS on F2 cars, active aero and
 * overtake mode on cars before 2026).
 */
function used(d: CompareData, channel: string): boolean {
	return d.laps.some((l) => l.channels[channel]?.some((v) => v != null && v !== 0));
}

const onOff = (v: number) => (v >= 0.5 ? 'on' : 'off');

/** A stepped on/off chart of a flag channel. */
function flagPanel(key: string, title: string, channel: string): Panel {
	return {
		key,
		title,
		unit: '',
		height: 70,
		stepped: true,
		discrete: true,
		lines: [{ channel, scale: (v) => v }],
		range: [0, 1],
		format: onOff
	};
}

/** Diff to ref, speed, throttle, brake, gear and steering (those the laps have). */
export function mainPanels(d: CompareData): Panel[] {
	const list: Panel[] = [];
	if (d.laps.length > 1)
		list.push({
			key: 'delta',
			title: 'Diff to ref',
			unit: 's',
			height: 140,
			lines: [],
			delta: true,
			format: formatDeltaS
		});
	if (has(d, 'speed'))
		list.push({
			key: 'speed',
			title: 'Speed',
			unit: 'km/h',
			height: 170,
			lines: [{ channel: 'speed', scale: (v) => v * 3.6 }],
			format: fixed(0, 'km/h')
		});
	for (const [channel, title] of [
		['throttle', 'Throttle'],
		['brake', 'Brake']
	] as const)
		if (has(d, channel))
			list.push({
				key: channel,
				title,
				unit: '%',
				height: 110,
				lines: [{ channel, scale: (v) => v * 100 }],
				range: [0, 100],
				format: fixed(0, '%')
			});
	if (has(d, 'gear')) {
		// from the lowest to the highest gear driven, a label per gear
		const gears = d.laps.flatMap((l) => (l.channels.gear ?? []).filter((g) => g != null));
		const lo = Math.min(0, ...gears);
		const hi = Math.max(1, ...gears);
		list.push({
			key: 'gear',
			title: 'Gear',
			unit: '',
			height: 140,
			stepped: true,
			discrete: true,
			lines: [{ channel: 'gear', scale: (v) => v }],
			range: [lo - 0.4, hi + 0.4],
			format: formatGear
		});
	}
	if (has(d, 'steer'))
		list.push({
			key: 'steer',
			title: 'Steering',
			unit: '°',
			height: 120,
			lines: [{ channel: 'steer', scale: (v) => v * RAD_TO_DEG }],
			format: (v) => `${v.toFixed(0)}°`
		});
	else if (has(d, 'steer_norm'))
		// games that report only the input (F1): -100 % full left … 100 % full right
		list.push({
			key: 'steer',
			title: 'Steering',
			unit: '%',
			height: 120,
			lines: [{ channel: 'steer_norm', scale: (v) => v * 100 }],
			range: [-100, 100],
			format: fixed(0, '%')
		});
	return list;
}

/** Further channels, below the main charts ("More channels"). */
export function morePanels(d: CompareData): Panel[] {
	const list: Panel[] = [];
	if (has(d, 'rpm'))
		list.push({
			key: 'rpm',
			title: 'RPM',
			unit: 'rpm',
			height: 110,
			lines: [{ channel: 'rpm', scale: (v) => v }],
			format: fixed(0, 'rpm')
		});
	// Le Mans Ultimate: virtual energy (solid) and battery (dashed)
	const energy = [
		{ channel: 'lmu.virtual_energy', scale: (v: number) => v * 100, name: 'VE' },
		{ channel: 'lmu.battery', scale: (v: number) => v * 100, dash: true, name: 'battery' }
	].filter((l) => has(d, l.channel));
	if (energy.length > 0)
		list.push({
			key: 'energy',
			title: 'Virtual energy / battery',
			unit: '%',
			height: 110,
			lines: energy,
			format: fixed(1, '%')
		});
	for (const [channel, title] of [
		['abs_active', 'ABS active'],
		['tc_active', 'TC active']
	] as const)
		if (has(d, channel)) list.push(flagPanel(channel, title, channel));
	// driver aid settings (TC solid, ABS dashed) and the motor map
	const aids = [
		{ channel: 'lmu.tc_level', scale: (v: number) => v, name: 'TC' },
		{ channel: 'lmu.abs_level', scale: (v: number) => v, dash: true, name: 'ABS' }
	].filter((l) => has(d, l.channel));
	if (aids.length > 0)
		list.push({
			key: 'aids',
			title: 'TC / ABS level',
			unit: '',
			height: 90,
			stepped: true,
			lines: aids,
			format: fixed(0, '')
		});
	if (has(d, 'lmu.motor_map'))
		list.push({
			key: 'motor_map',
			title: 'Motor map',
			unit: '',
			height: 80,
			stepped: true,
			lines: [{ channel: 'lmu.motor_map', scale: (v) => v }],
			format: fixed(0, '')
		});
	list.push(...f1Panels(d));
	return list;
}

/**
 * F1 25: ERS, DRS (cars before 2026, also F2), active aero and overtake mode
 * (2026 cars). Each only if one of the laps' cars has it.
 */
function f1Panels(d: CompareData): Panel[] {
	const list: Panel[] = [];
	const cars2026 = used(d, 'f1.active_aero_mode') || used(d, 'f1.overtake_active');
	if (used(d, 'f1.ers_deploy_mode') || used(d, 'f1.ers_power') || used(d, 'f1.ers_store')) {
		// mode 3 is "overtake" before 2026 and "boost" with the 2026 cars
		const top = !cars2026 ? 'overtake' : used(d, 'f1.drs') ? 'overtake/boost' : 'boost';
		const modes = ['none', 'medium', 'hotlap', top];
		if (has(d, 'f1.ers_deploy_mode'))
			list.push({
				key: 'ers_mode',
				title: 'ERS mode',
				unit: '',
				height: 90,
				stepped: true,
				discrete: true,
				lines: [{ channel: 'f1.ers_deploy_mode', scale: (v) => v }],
				range: [-0.4, 3.4],
				format: (v) => modes[Math.round(v)] ?? fixed(0, '')(v)
			});
		if (has(d, 'f1.ers_power'))
			list.push({
				key: 'ers_power',
				title: 'ERS power',
				unit: 'kW',
				height: 110,
				lines: [{ channel: 'f1.ers_power', scale: (v) => v / 1000 }],
				format: fixed(0, 'kW')
			});
		// the store holds 4 MJ
		if (has(d, 'f1.ers_store'))
			list.push({
				key: 'ers_store',
				title: 'ERS store',
				unit: 'MJ',
				height: 110,
				lines: [{ channel: 'f1.ers_store', scale: (v) => v / 1e6 }],
				range: [0, 4],
				format: fixed(2, 'MJ')
			});
	}
	if (used(d, 'f1.drs')) list.push(flagPanel('drs', 'DRS', 'f1.drs'));
	if (cars2026) {
		list.push(flagPanel('straight_mode', 'Straight mode', 'f1.active_aero_mode'));
		list.push(flagPanel('overtake', 'Overtake', 'f1.overtake_active'));
	}
	return list;
}

/**
 * The series of a panel: one per lap and line, with its lap index. The diff
 * chart has the reference as the zero line.
 */
export function panelSeries(
	d: CompareData,
	p: Panel
): { lap: number; line?: PanelLine; values: (number | null)[] }[] {
	const out: { lap: number; line?: PanelLine; values: (number | null)[] }[] = [];
	d.laps.forEach((lap, k) => {
		if (p.delta) {
			out.push({ lap: k, values: k === 0 ? d.distM.map(() => 0) : lap.deltaS });
			return;
		}
		for (const line of p.lines) {
			const src = lap.channels[line.channel];
			if (src)
				out.push({ lap: k, line, values: src.map((v) => (v == null ? null : line.scale(v))) });
		}
	});
	return out;
}
