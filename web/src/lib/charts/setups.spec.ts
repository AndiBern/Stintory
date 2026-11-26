// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from 'vitest';
import type { LapSetup, LapTrace, SetupItem } from '#lib/api/index.ts';
import { differences, settingCount, setupGroups, setupStatus, setupSummary } from './setups.ts';

function lap(setup: LapSetup | null, setupHidden = false, fixedSetup = false): LapTrace {
	return {
		lapId: crypto.randomUUID(),
		sessionId: 's',
		number: 1,
		lapTimeMs: 90_000,
		valid: true,
		channels: {},
		deltaS: [],
		setup,
		setupHidden,
		fixedSetup
	};
}

function setup(items: [string, string, string][], source: 'lap' | 'session' = 'lap'): LapSetup {
	return {
		source,
		items: items.map(([group, name, value]): SetupItem => ({ group, name, value }))
	};
}

describe('setupGroups', () => {
	it('puts the laps side by side and finds the differences', () => {
		const groups = setupGroups([
			lap(
				setup([
					['Aerodynamics', 'Rear wing', '8'],
					['Tires', 'Pressure FL', '136 kPa'],
					['Tires', 'Pressure FR', '136 kPa']
				])
			),
			lap(
				setup([
					['Aerodynamics', 'Rear wing', '9'],
					['Tires', 'Pressure FL', '136 kPa'],
					['Tires', 'Pressure FR', '136 kPa'],
					['Brakes', 'Brake balance', '52:48']
				])
			)
		]);
		expect(groups.map((g) => g.name)).toEqual(['Aerodynamics', 'Tires', 'Brakes']);
		expect(groups[0].rows[0]).toEqual({
			group: 'Aerodynamics',
			name: 'Rear wing',
			values: ['8', '9'],
			differs: true
		});
		expect(groups[1].rows.map((r) => r.differs)).toEqual([false, false]);
		// a setting only one lap has is a difference
		expect(groups[2].rows[0]).toMatchObject({ values: [null, '52:48'], differs: true });
		expect(settingCount(groups)).toBe(4);
	});

	it('compares only laps that have a setup', () => {
		const groups = setupGroups([
			lap(setup([['Aerodynamics', 'Rear wing', '8']])),
			lap(null, true),
			lap(setup([['Aerodynamics', 'Rear wing', '8']], 'session'))
		]);
		expect(groups[0].rows[0]).toMatchObject({ values: ['8', null, '8'], differs: false });
	});

	it('is empty without setups', () => {
		expect(setupGroups([lap(null), lap(null)])).toEqual([]);
	});
});

describe('differences', () => {
	it('keeps the rows that differ and their groups', () => {
		const groups = setupGroups([
			lap(
				setup([
					['Aerodynamics', 'Rear wing', '8'],
					['Tires', 'Pressure FL', '136 kPa']
				])
			),
			lap(
				setup([
					['Aerodynamics', 'Rear wing', '8'],
					['Tires', 'Pressure FL', '140 kPa']
				])
			)
		]);
		const diff = differences(groups);
		expect(diff.map((g) => [g.name, g.rows.map((r) => r.name)])).toEqual([
			['Tires', ['Pressure FL']]
		]);
	});
});

describe('setupSummary', () => {
	const wing = (v: string) => setup([['Aerodynamics', 'Rear wing', v]]);

	it('says whether the setups differ', () => {
		expect(setupSummary([lap(wing('8')), lap(wing('9'))])).toBe('1 difference');
		expect(setupSummary([lap(wing('8')), lap(wing('8'))])).toBe('Same setup');
		expect(setupSummary([lap(wing('8')), lap(wing('8')), lap(null, true)])).toBe(
			'Same setup where known'
		);
		expect(setupSummary([lap(wing('8')), lap(null)])).toBe('Only one lap has a setup');
		expect(setupSummary([lap(null), lap(null)])).toBe('No setup known for these laps');
		expect(setupSummary([lap(wing('8'))])).toBe('1 setting');
	});

	it('adds which laps had a fixed setup', () => {
		expect(setupSummary([lap(wing('8'), false, true), lap(null, true, true)])).toBe(
			'Only one lap has a setup · all fixed setups'
		);
		expect(setupSummary([lap(wing('8'), false, true), lap(wing('9'))])).toBe(
			'1 difference · 1 of 2 fixed setups'
		);
		expect(setupSummary([lap(wing('8'), false, true)])).toBe('1 setting · fixed setup');
	});
});

describe('setupStatus', () => {
	it('tells where the setup comes from or why there is none', () => {
		expect(setupStatus(lap(setup([['A', 'B', 'C']])))).toBe('lap');
		expect(setupStatus(lap(setup([['A', 'B', 'C']], 'session')))).toBe('session');
		expect(setupStatus(lap(null, true))).toBe('hidden');
		expect(setupStatus(lap(null))).toBe('none');
	});
});
