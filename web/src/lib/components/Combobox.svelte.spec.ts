// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { page, userEvent } from 'vitest/browser';
import Combobox from './Combobox.svelte';

const options = ['Monza', 'Nürburgring Nordschleife', 'Spa-Francorchamps', 'Suzuka'];

function setup(value = '') {
	const onchange = vi.fn();
	render(Combobox, { label: 'Track', placeholder: 'All tracks', options, value, onchange });
	return { onchange, field: page.getByRole('combobox', { name: 'Track' }) };
}

describe('Combobox', () => {
	it('filters the options while typing and picks one with Enter', async () => {
		const { onchange, field } = setup();
		await field.fill('nurb');
		await expect.element(page.getByRole('option')).toHaveTextContent('Nürburgring Nordschleife');
		await userEvent.keyboard('{Enter}');
		expect(onchange).toHaveBeenCalledWith('Nürburgring Nordschleife');
		await expect.element(field).toHaveValue('Nürburgring Nordschleife');
		await expect.element(page.getByRole('listbox')).not.toBeInTheDocument();
	});

	it('shows all options on click and picks one with the mouse', async () => {
		const { onchange, field } = setup('Monza');
		await field.click();
		expect(page.getByRole('option').all()).toHaveLength(4);
		await page.getByRole('option', { name: 'Suzuka' }).click();
		expect(onchange).toHaveBeenCalledWith('Suzuka');
		await expect.element(field).toHaveValue('Suzuka');
	});

	it('moves through the options with the arrow keys', async () => {
		const { onchange, field } = setup();
		await field.click();
		await userEvent.keyboard('{ArrowDown}{ArrowDown}{Enter}');
		expect(onchange).toHaveBeenCalledWith('Nürburgring Nordschleife');
	});

	it('keeps the old value when the typed text names no option', async () => {
		const { onchange, field } = setup('Monza');
		await field.fill('xyz');
		await expect.element(page.getByText('No matches')).toBeVisible();
		await userEvent.keyboard('{Escape}');
		await expect.element(field).toHaveValue('Monza');
		await field.fill('spa');
		await userEvent.tab();
		await expect.element(field).toHaveValue('Monza');
		expect(onchange).not.toHaveBeenCalled();
	});

	it('takes a full name typed without picking it', async () => {
		const { onchange, field } = setup();
		await field.fill('suzuka');
		await userEvent.tab();
		expect(onchange).toHaveBeenCalledWith('Suzuka');
	});

	it('clears with the button or by emptying the text', async () => {
		const { onchange, field } = setup('Monza');
		await page.getByRole('button', { name: 'Clear track' }).click();
		expect(onchange).toHaveBeenLastCalledWith('');
		await expect.element(field).toHaveValue('');
		await field.fill('Suzuka');
		await userEvent.keyboard('{Enter}');
		await field.fill('');
		await userEvent.tab();
		expect(onchange).toHaveBeenLastCalledWith('');
	});
});
