// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

/** The lap table's weather, fuel and tyre columns. */

import type { GameId, LapConditions, LapSummary, Wheels } from './api/index.ts';

export type ConditionKey = 'airTemp' | 'trackTemp' | 'humidity' | 'weather' | 'fuel' | 'tires';

/** Icons: the columns' and the tyre details'. */
export type IconName = ConditionKey | 'tyreTemp' | 'tyreWear';

export interface ConditionColumn {
	key: ConditionKey;
	/** Header tooltip (the header itself is an icon) */
	title: string;
}

export const CONDITION_COLUMNS: ConditionColumn[] = [
	{ key: 'airTemp', title: 'Air temperature (average over the lap)' },
	{ key: 'trackTemp', title: 'Track temperature (average over the lap)' },
	{ key: 'humidity', title: 'Humidity' },
	{ key: 'weather', title: 'Weather' },
	{ key: 'fuel', title: 'Fuel at the start of the lap' },
	{
		key: 'tires',
		title:
			'Tires: compound, temperature (average over the lap) and wear (end of the lap) of all four'
	}
];

/** The value behind a column, null if the lap has none. */
function value(c: LapConditions, key: ConditionKey): unknown {
	switch (key) {
		case 'airTemp':
			return c.airTempC;
		case 'trackTemp':
			return c.trackTempC;
		case 'humidity':
			return c.humidity;
		case 'weather':
			return c.weather;
		case 'fuel':
			return c.fuelStart;
		case 'tires':
			return c.tyreCompound ?? c.tyreTempC ?? c.tyreWear;
	}
}

/** Columns with a value in at least one lap; the game doesn't provide the others. */
export function visibleColumns(laps: LapSummary[]): ConditionColumn[] {
	return CONDITION_COLUMNS.filter((col) =>
		laps.some((l) => l.conditions && value(l.conditions, col.key) != null)
	);
}

export function average(w: Wheels): number {
	return (w.fl + w.fr + w.rl + w.rr) / 4;
}

export function formatTemp(c: number): string {
	return `${c.toFixed(1)} °C`;
}

/** Wear as worn percent, e.g. `4.2 %`. */
export function formatWear(ratio: number): string {
	return `${(ratio * 100).toFixed(1)} %`;
}

/** Cell text; empty without a value. */
export function cellText(c: LapConditions | null | undefined, key: ConditionKey): string {
	if (!c) return '';
	switch (key) {
		case 'airTemp':
			return c.airTempC == null ? '' : formatTemp(c.airTempC);
		case 'trackTemp':
			return c.trackTempC == null ? '' : formatTemp(c.trackTempC);
		case 'humidity':
			return c.humidity == null ? '' : `${Math.round(c.humidity * 100)} %`;
		case 'weather':
			return c.weather ?? '';
		case 'fuel':
			return c.fuelStart == null ? '' : `${c.fuelStart.toFixed(1)} ${c.fuelUnit ?? ''}`.trim();
		case 'tires': {
			const t = tireSummary(c);
			return [t.compound, t.temp, t.wear].filter(Boolean).join(' ');
		}
	}
}

/** Fuel used in the lap, for the fuel cell's tooltip. */
export function fuelUsedText(c: LapConditions | null | undefined): string | undefined {
	if (c?.fuelUsed == null) return undefined;
	return `Used ${c.fuelUsed.toFixed(2)} ${c.fuelUnit ?? ''} in this lap`.trim();
}

/** Some tyre went flat in the lap. */
export function anyFlat(c: LapConditions | null | undefined): boolean {
	const f = c?.tyreFlat;
	return !!f && (f.fl || f.fr || f.rl || f.rr);
}

/** A tyre temperature, e.g. `55 °C`. */
export function formatTyreTemp(c: number): string {
	return `${Math.round(c)} °C`;
}

/**
 * The tires cell: compound, temperature and wear of all four (averages);
 * null where the game doesn't say. "Mixed" if the four tires aren't all
 * the same compound or were changed during the lap.
 */
export function tireSummary(c: LapConditions): {
	compound: string | null;
	temp: string | null;
	wear: string | null;
} {
	const t = c.tyreCompound;
	return {
		compound: !t ? null : new Set([t.fl, t.fr, t.rl, t.rr]).size > 1 || t.before ? 'Mixed' : t.fl,
		temp: c.tyreTempC ? formatTyreTemp(average(c.tyreTempC)) : null,
		wear: c.tyreWear ? formatWear(average(c.tyreWear)) : null
	};
}

export interface WheelDetails {
	key: keyof Wheels;
	label: string;
	/** e.g. `Soft (C5) – 5 laps`, `Soft → Hard` after a change in the lap */
	compound: string | null;
	/** the compound's name (at the end of the lap), for its colour */
	compoundName: string | null;
	temp: string | null;
	wear: string | null;
	flat: boolean;
}

const WHEEL_LABELS: [keyof Wheels, string][] = [
	['fl', 'Front left'],
	['fr', 'Front right'],
	['rl', 'Rear left'],
	['rr', 'Rear right']
];

/** All four tires for the tooltip; only what the game provides. */
export function wheelDetails(c: LapConditions): WheelDetails[] {
	const t = c.tyreCompound;
	return WHEEL_LABELS.map(([key, label]) => {
		const name = t ? t[key] : null;
		const before = t?.before?.[key];
		let compound = before && before !== name ? `${before} → ${name}` : name;
		if (name && t?.detail) compound += ` (${t.detail})`;
		if (name && t?.ageLaps != null)
			compound += ` – ${t.ageLaps} ${t.ageLaps === 1 ? 'lap' : 'laps'}`;
		return {
			key,
			label,
			compound,
			compoundName: name,
			temp: c.tyreTempC ? formatTyreTemp(c.tyreTempC[key]) : null,
			wear: c.tyreWear ? formatWear(c.tyreWear[key]) : null,
			flat: !!c.tyreFlat?.[key]
		};
	});
}

type Colour = 'red' | 'yellow' | 'white' | 'green' | 'blue' | 'grey';

const COLOUR_CLASSES: Record<Colour, { bg: string; text: string }> = {
	red: { bg: 'bg-red-500', text: 'text-red-500' },
	yellow: { bg: 'bg-yellow-400', text: 'text-yellow-400' },
	white: { bg: 'bg-zinc-100', text: 'text-zinc-100' },
	green: { bg: 'bg-green-500', text: 'text-green-500' },
	blue: { bg: 'bg-blue-500', text: 'text-blue-500' },
	grey: { bg: 'bg-zinc-500', text: 'text-zinc-400' }
};

/**
 * Each game's own compound colours, so the dot means what it means in the
 * game. F1: soft red, medium yellow, hard white, intermediate green, wet
 * blue. LMU: soft white, medium yellow, hard red, wet blue. iRacing: wet
 * blue, dry compounds neutral (colours not known yet).
 */
const GAME_COLOURS: Record<GameId, Partial<Record<string, Colour>>> = {
	f1_25: { soft: 'red', medium: 'yellow', hard: 'white', inter: 'green', wet: 'blue' },
	lmu: { soft: 'white', medium: 'yellow', hard: 'red', wet: 'blue' },
	iracing: { wet: 'blue' }
};

/**
 * The colour of a compound in the game; grey for others and "Mixed". The
 * name is always shown next to it.
 */
export function compoundColor(name: string | null, game: GameId): { bg: string; text: string } {
	const n = (name ?? '').toLowerCase();
	const kind = ['soft', 'medium', 'hard', 'inter', 'wet'].find((k) => n.includes(k));
	const colour = (kind && GAME_COLOURS[game][kind]) || (n.includes('rain') ? 'blue' : 'grey');
	return COLOUR_CLASSES[colour];
}

export interface CompoundDot {
	/** which tires it stands for */
	label: string;
	/** the compound at the end of the lap */
	after: { name: string; bg: string };
	/** the compound before, if it was changed in the lap (left half) */
	before: { name: string; bg: string } | null;
}

/**
 * The dots of the tires cell. One if all four tires are the same, two
 * (front, rear) if each axle is even, else four (FL, FR, RL, RR). Tires
 * count as the same if they had the same compound before and after the lap,
 * so a single changed tire gives four dots. A dot is split (before | after)
 * where the compound was changed in the lap.
 */
export function compoundDots(c: LapConditions, game: GameId): CompoundDot[] {
	const t = c.tyreCompound;
	if (!t) return [];
	const pair = (k: keyof Wheels) => ({ before: t.before?.[k] ?? t[k], after: t[k] });
	const same = (a: keyof Wheels, b: keyof Wheels) =>
		pair(a).before === pair(b).before && pair(a).after === pair(b).after;
	const groups: [keyof Wheels, string][] =
		same('fl', 'fr') && same('rl', 'rr') && same('fl', 'rl')
			? [['fl', 'All tires']]
			: same('fl', 'fr') && same('rl', 'rr')
				? [
						['fl', 'Front'],
						['rl', 'Rear']
					]
				: [
						['fl', 'Front left'],
						['fr', 'Front right'],
						['rl', 'Rear left'],
						['rr', 'Rear right']
					];
	return groups.map(([k, label]) => {
		const { before, after } = pair(k);
		return {
			label,
			after: { name: after, bg: compoundColor(after, game).bg },
			before: before !== after ? { name: before, bg: compoundColor(before, game).bg } : null
		};
	});
}
