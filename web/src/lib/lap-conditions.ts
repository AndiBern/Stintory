// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

/** The lap table's weather, fuel and tyre columns. */

import type { LapConditions, LapSummary, Wheels } from './api/index.ts';

export type ConditionKey =
	'airTemp' | 'trackTemp' | 'humidity' | 'weather' | 'fuel' | 'tyreTemp' | 'tyreWear';

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
