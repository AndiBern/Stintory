// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

/** Lower case without accents, so "nurb" finds "Nürburgring". */
export function fold(text: string): string {
	return text.normalize('NFD').replace(/\p{M}/gu, '').toLowerCase();
}

/**
 * The options containing every word of the query, in their original order.
 * "spa 24" finds "Spa-Francorchamps 24h"; an empty query keeps all options.
 */
export function matchOptions(options: string[], query: string): string[] {
	const words = fold(query).split(/\s+/).filter(Boolean);
	if (words.length === 0) return options;
	return options.filter((option) => {
		const name = fold(option);
		return words.every((w) => name.includes(w));
	});
}

/** The option the typed text names exactly, ignoring case and accents. */
export function exactOption(options: string[], text: string): string | undefined {
	const wanted = fold(text.trim());
	return options.find((o) => fold(o) === wanted);
}
