// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * Colours of compared laps; index 0 is the reference (almost white, the
 * neutral baseline). The others are a categorical palette validated for the
 * dark surface (lightness band, colour-vision deficiency separation, ≥ 3:1
 * contrast); lap labels and line keys carry identity too.
 */
export const LAP_COLORS = [
	'#f4f4f5', // reference
	'#3987e5', // blue
	'#d95926', // orange
	'#199e70', // aqua
	'#c98500', // yellow
	'#d55181', // magenta
	'#008300', // green
	'#9085e9' // violet
];

export function lapColor(index: number): string {
	return LAP_COLORS[index % LAP_COLORS.length];
}
