// SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { expect, test } from '@playwright/test';

test('shows the sessions page with navigation', async ({ page }) => {
	await page.goto('/');
	await expect(page.getByRole('heading', { name: 'Sessions' })).toBeVisible();
	await expect(page.getByRole('navigation', { name: 'Main' })).toBeVisible();
});

test('navigates to the about page', async ({ page }) => {
	// in the browser (no desktop app) the settings page only has "About"
	await page.goto('/');
	await page.getByRole('link', { name: 'About' }).click();
	await expect(page.getByRole('heading', { name: 'About', level: 1 })).toBeVisible();
});
