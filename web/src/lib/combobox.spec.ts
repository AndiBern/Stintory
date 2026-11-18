// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from 'vitest';
import { exactOption, matchOptions } from './combobox.ts';

const tracks = ['Autódromo José Carlos Pace', 'Monza', 'Nürburgring Nordschleife', 'Spa 24h'];

describe('matchOptions', () => {
	it('keeps all options for an empty query', () => {
		expect(matchOptions(tracks, '  ')).toEqual(tracks);
	});

	it('matches anywhere in the name, ignoring case and accents', () => {
		expect(matchOptions(tracks, 'NURB')).toEqual(['Nürburgring Nordschleife']);
		expect(matchOptions(tracks, 'jose')).toEqual(['Autódromo José Carlos Pace']);
		expect(matchOptions(tracks, 'O')).toEqual(tracks.slice(0, 3));
	});

	it('needs every word, in any order', () => {
		expect(matchOptions(tracks, '24 spa')).toEqual(['Spa 24h']);
		expect(matchOptions(tracks, 'spa monza')).toEqual([]);
	});
});

describe('exactOption', () => {
	it('finds the option the text names', () => {
		expect(exactOption(tracks, ' monza ')).toBe('Monza');
		expect(exactOption(tracks, 'nurburgring nordschleife')).toBe('Nürburgring Nordschleife');
	});

	it('ignores partial names', () => {
		expect(exactOption(tracks, 'Mon')).toBeUndefined();
	});
});
