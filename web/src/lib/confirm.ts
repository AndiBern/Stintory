// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * Asks the user to confirm an action. Always use this instead of `confirm()`:
 * in the desktop app Tauri's dialog plugin replaces `window.confirm` with an
 * async version, so `if (!confirm(...))` sees a Promise (truthy) and goes
 * ahead without waiting for the answer.
 */
export async function confirmAction(message: string): Promise<boolean> {
	return await (window.confirm(message) as boolean | Promise<boolean>);
}
