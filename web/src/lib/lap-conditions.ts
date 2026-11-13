// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

/** The lap table's weather, fuel and tyre columns. */

import type { LapConditions, LapSummary, Wheels } from './api/index.ts';

export type ConditionKey =
	'airTemp' | 'trackTemp' | 'humidity' | 'weather' | 'fuel' | 'tyres' | 'tyreTemp' | 'tyreWear';

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
	{ key: 'tyres', title: 'Tyre compound' },
	{ key: 'tyreTemp', title: 'Tyre temperature (average of all four over the lap)' },
	{ key: 'tyreWear', title: 'Tyre wear (average of all four at the end of the lap)' }
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
		case 'tyres':
			return c.tyreCompound;
		case 'tyreTemp':
			return c.tyreTempC;
		case 'tyreWear':
			return c.tyreWear;
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
		case 'tyres': {
			const t = c.tyreCompound;
			if (!t) return '';
			return t.changedFrom ? `${t.changedFrom} → ${t.name}` : t.name;
		}
		case 'tyreTemp':
			return c.tyreTempC ? `${average(c.tyreTempC).toFixed(0)} °C` : '';
		case 'tyreWear':
			return c.tyreWear ? formatWear(average(c.tyreWear)) : '';
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

/** Tooltip of the compound cell: the details the game has. */
export function compoundDetails(c: LapConditions | null | undefined): string | undefined {
	const t = c?.tyreCompound;
	if (!t) return undefined;
	const parts: string[] = [];
	if (t.detail) parts.push(t.detail);
	if (t.ageLaps != null) parts.push(t.ageLaps === 1 ? '1 lap old' : `${t.ageLaps} laps old`);
	if (t.changedFrom) parts.push(`changed from ${t.changedFrom} in this lap`);
	return parts.length > 0 ? parts.join(' · ') : undefined;
}

/**
 * The usual colour of a compound (soft red, medium yellow, hard white,
 * intermediate green, wet blue) as a class; grey for others. The name is
 * always shown next to it.
 */
export function compoundColor(name: string): string {
	const n = name.toLowerCase();
	if (n.includes('soft')) return 'bg-red-500';
	if (n.includes('medium')) return 'bg-yellow-400';
	if (n.includes('hard')) return 'bg-zinc-100';
	if (n.includes('inter')) return 'bg-green-500';
	if (n.includes('wet') || n.includes('rain')) return 'bg-blue-500';
	return 'bg-zinc-500';
}
