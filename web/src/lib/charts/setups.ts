// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The car setups of compared laps side by side: one row per setting, one
 * value per lap, and whether the laps differ there.
 */

import type { LapTrace } from '#lib/api/index.ts';

/** What is known about a lap's setup. */
export type SetupStatus =
	/** recorded with the lap */
	| 'lap'
	/** the last setup of the lap's session (older laps) */
	| 'session'
	/** the owner doesn't share it */
	| 'hidden'
	/** none recorded, or left out of an export */
	| 'none';

export function setupStatus(lap: LapTrace): SetupStatus {
	if (lap.setup) return lap.setup.source;
	return lap.setupHidden ? 'hidden' : 'none';
}

export interface SetupRow {
	group: string;
	name: string;
	/** per lap; null where the lap has no setup or not this setting */
	values: (string | null)[];
	/** the laps with a setup don't all have the same value */
	differs: boolean;
}

export interface SetupGroup {
	name: string;
	rows: SetupRow[];
}

/**
 * The settings of all laps' setups, grouped, in the order the reference
 * (then the other laps) has them.
 */
export function setupGroups(laps: LapTrace[]): SetupGroup[] {
	const groups = new Map<string, Map<string, (string | null)[]>>();
	laps.forEach((lap, i) => {
		for (const item of lap.setup?.items ?? []) {
			let rows = groups.get(item.group);
			if (!rows) groups.set(item.group, (rows = new Map()));
			let values = rows.get(item.name);
			if (!values) rows.set(item.name, (values = laps.map(() => null)));
			values[i] = item.value;
		}
	});
	const withSetup = laps.map((l) => !!l.setup);
	return [...groups].map(([group, rows]) => ({
		name: group,
		rows: [...rows].map(([name, values]) => {
			const seen = new Set(values.filter((_, i) => withSetup[i]));
			return { group, name, values, differs: seen.size > 1 };
		})
	}));
}

/** Only the rows where the laps differ; groups without any are left out. */
export function differences(groups: SetupGroup[]): SetupGroup[] {
	return groups
		.map((g) => ({ ...g, rows: g.rows.filter((r) => r.differs) }))
		.filter((g) => g.rows.length > 0);
}

/**
 * One line about the laps' setups, e.g. `2 differences`, `Same setup · all
 * fixed setups` or `12 settings` for a single lap.
 */
export function setupSummary(laps: LapTrace[]): string {
	const comparing = laps.length > 1;
	const known = laps.filter((l) => l.setup).length;
	const groups = setupGroups(laps);
	const diff = settingCount(differences(groups));
	let text: string;
	if (known === 0) text = 'No setup known for these laps';
	else if (!comparing) {
		const n = settingCount(groups);
		text = `${n} ${n === 1 ? 'setting' : 'settings'}`;
	} else if (known === 1) text = 'Only one lap has a setup';
	else if (diff === 0) text = known === laps.length ? 'Same setup' : 'Same setup where known';
	else text = `${diff} ${diff === 1 ? 'difference' : 'differences'}`;
	const fixed = laps.filter((l) => l.fixedSetup).length;
	if (fixed === 0) return text;
	if (fixed === laps.length) return `${text} · ${comparing ? 'all fixed setups' : 'fixed setup'}`;
	return `${text} · ${fixed} of ${laps.length} fixed setups`;
}

/** Number of settings in the groups. */
export function settingCount(groups: SetupGroup[]): number {
	return groups.reduce((n, g) => n + g.rows.length, 0);
}
