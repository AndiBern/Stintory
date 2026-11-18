// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

import { expect, test, type Page } from '@playwright/test';

function session(id: string, trackName: string, day: number) {
	return {
		id,
		game: 'iracing',
		trackKey: trackName.toLowerCase(),
		trackName,
		carKey: 'gt3',
		carName: 'GT3',
		sessionType: 'practice',
		startedAt: `2026-11-${String(day).padStart(2, '0')}T18:00:00Z`,
		lapCount: 5,
		bestLapMs: 140_000,
		origin: { kind: 'own' }
	};
}

/** Serves `sessions` as the session list; change the array between page loads. */
async function serve(page: Page, sessions: ReturnType<typeof session>[]) {
	await page.route('**/api/v1/sessions', (route) => route.fulfill({ json: sessions }));
}

const notice = (page: Page) => page.getByText(/new sessions? hidden by the filters/);

test('points out new sessions the filters hide after a restart', async ({ page }) => {
	const sessions = [session('a', 'Monza', 1), session('b', 'Spa', 2)];
	await serve(page, sessions);
	await page.goto('/');

	// first visit: everything counts as seen
	await page.getByRole('combobox', { name: 'Track' }).fill('monza');
	await page.keyboard.press('Enter');
	await expect(page.getByText('1 of 2 sessions')).toBeVisible();
	await expect(notice(page)).toHaveCount(0);

	// a session the filter hides arrives while the app is closed
	sessions.push(session('c', 'Spa', 3));
	await page.reload();
	await expect(page.getByText('1 of 3 sessions')).toBeVisible();
	await expect(notice(page)).toHaveText('1 new session hidden by the filters');

	// dismissed: gone, and stays gone after a restart
	await page.getByRole('button', { name: 'Dismiss' }).click();
	await expect(notice(page)).toHaveCount(0);
	await page.reload();
	await expect(page.getByText('1 of 3 sessions')).toBeVisible();
	await expect(notice(page)).toHaveCount(0);

	// only what came after dismissing counts
	sessions.push(session('d', 'Spa', 4));
	await page.reload();
	await expect(notice(page)).toHaveText('1 new session hidden by the filters');

	// a new session the filter shows is not pointed out
	sessions.push(session('e', 'Monza', 5));
	await page.reload();
	await expect(page.getByText('2 of 5 sessions')).toBeVisible();
	await expect(notice(page)).toHaveText('1 new session hidden by the filters');

	// Show clears the filters and lights up the session it revealed for a moment
	await page.getByRole('button', { name: 'Show', exact: true }).click();
	await expect(notice(page)).toHaveCount(0);
	const lit = page.locator('tbody tr.animate-flash-new');
	await expect(lit).toHaveCount(1);
	await expect(lit).toContainText('Spa');
	await expect(lit).toHaveCount(0, { timeout: 4000 });
	await expect(page.getByRole('combobox', { name: 'Track' })).toHaveValue('');
	await expect(page.getByRole('button', { name: 'Clear filters' })).toHaveCount(0);
	await expect(page.locator('tbody tr')).toHaveCount(5);

	// filtering again later does not bring the notice back
	await page.getByRole('combobox', { name: 'Track' }).fill('monza');
	await page.keyboard.press('Enter');
	await page.reload();
	await expect(page.getByText('2 of 5 sessions')).toBeVisible();
	await expect(notice(page)).toHaveCount(0);
});
