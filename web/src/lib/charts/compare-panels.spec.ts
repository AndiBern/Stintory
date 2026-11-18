// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from 'vitest';
import type { CompareData, LapTrace } from '#lib/api/index.ts';
import { morePanels } from './compare-panels.ts';

function lap(channels: Record<string, (number | null)[]>): LapTrace {
	return {
		lapId: 'l',
		sessionId: 's',
		number: 1,
		lapTimeMs: null,
		valid: true,
		channels,
		deltaS: [0, 0, 0]
	};
}

const data = (...laps: LapTrace[]): CompareData => ({
	stepM: 1,
	distM: [0, 1, 2],
	units: {},
	laps
});

// what the game sends for each kind of car (seen in recordings)
const f1_2025 = lap({
	'f1.ers_deploy_mode': [2, 2, 2],
	'f1.ers_power': [0, 120_000, 120_000],
	'f1.ers_store': [4e6, 4e6, 4e6],
	'f1.drs': [0, 1, 1],
	'f1.active_aero_mode': [0, 0, 0],
	'f1.overtake_active': [0, 0, 0]
});
const f1_26 = lap({
	'f1.ers_deploy_mode': [3, 3, 3],
	'f1.ers_power': [0, 350_000, 0],
	'f1.ers_store': [4e6, 3.5e6, 3e6],
	'f1.drs': [0, 0, 0],
	'f1.active_aero_mode': [0, 1, 1],
	'f1.overtake_active': [0, 0, 0]
});
const f2 = lap({
	'f1.ers_deploy_mode': [0, 0, 0],
	'f1.ers_power': [0, 0, 0],
	'f1.ers_store': [0, 0, 0],
	'f1.drs': [0, 1, null],
	'f1.active_aero_mode': [0, 0, 0],
	'f1.overtake_active': [0, 0, 0]
});

const keys = (d: CompareData) => morePanels(d).map((p) => p.key);

describe('F1 panels', () => {
	it('shows only what the cars have', () => {
		expect(keys(data(f1_2025))).toEqual(['ers_mode', 'ers_power', 'ers_store', 'drs']);
		expect(keys(data(f1_26))).toEqual([
			'ers_mode',
			'ers_power',
			'ers_store',
			'straight_mode',
			'overtake'
		]);
		expect(keys(data(f2))).toEqual(['drs']);
		expect(keys(data(f2, f1_26))).toEqual([
			'ers_mode',
			'ers_power',
			'ers_store',
			'drs',
			'straight_mode',
			'overtake'
		]);
	});

	it('names the ERS modes and scales the power to kW', () => {
		const mode = (d: CompareData) => morePanels(d).find((p) => p.key === 'ers_mode')!;
		expect([0, 1, 2, 3].map(mode(data(f1_2025)).format)).toEqual([
			'none',
			'medium',
			'hotlap',
			'overtake'
		]);
		expect(mode(data(f1_26)).format(3)).toBe('boost');
		expect(mode(data(f1_2025, f1_26)).format(3)).toBe('overtake/boost');
		const power = morePanels(data(f1_26)).find((p) => p.key === 'ers_power')!;
		expect(power.lines[0].scale(350_000)).toBe(350);
		expect(power.format(350)).toBe('350 kW');
		const store = morePanels(data(f1_26)).find((p) => p.key === 'ers_store')!;
		expect(store.format(store.lines[0].scale(3.5e6))).toBe('3.50 MJ');
	});

	it('keeps the ERS mode of laps recorded before the power channel', () => {
		const old = lap({ 'f1.ers_deploy_mode': [2, 2, 2], 'f1.drs': [0, 1, 1] });
		expect(keys(data(old))).toEqual(['ers_mode', 'drs']);
	});
});
