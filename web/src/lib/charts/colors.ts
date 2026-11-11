// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

/** Colours of compared laps; index 0 is the reference lap. */
export const LAP_COLORS = [
	'#f4f4f5', // reference: almost white
	'#ef4444',
	'#3b82f6',
	'#22c55e',
	'#eab308',
	'#a855f7',
	'#06b6d4',
	'#f97316'
];

export function lapColor(index: number): string {
	return LAP_COLORS[index % LAP_COLORS.length];
}
