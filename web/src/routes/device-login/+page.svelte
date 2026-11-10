<!-- SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { page } from '$app/state';
	import { accountApi } from '#lib/api/index.ts';
	import { auth, errorText } from '#lib/app-state.svelte.ts';

	// The desktop app opened this page (docs/PLAN.md §9.3): it waits on
	// redirect_uri (a loopback address) for the result.
	const params = page.url.searchParams;
	const redirectUri = params.get('redirect_uri') ?? '';
	const codeChallenge = params.get('code_challenge') ?? '';
	const appState = params.get('state') ?? '';
	const deviceName = params.get('device_name') || 'Desktop';

	const account = accountApi();
	const valid = /^http:\/\/(127\.0\.0\.1|\[::1\]|localhost):\d{4,5}\//.test(redirectUri);
	let busy = $state(false);
	let error = $state<string | null>(null);
	let done = $state<'allowed' | 'denied' | null>(null);

	async function allow() {
		if (!account) return;
		busy = true;
		error = null;
		try {
			const r = await account.approveDeviceLogin({
				redirectUri,
				codeChallenge,
				state: appState,
				deviceName
			});
			done = 'allowed';
			window.location.href = r.redirectTo;
		} catch (e) {
			error = errorText(e);
		} finally {
			busy = false;
		}
	}

	function deny() {
		done = 'denied';
		const sep = redirectUri.includes('?') ? '&' : '?';
		window.location.href = `${redirectUri}${sep}error=access_denied&state=${encodeURIComponent(appState)}`;
	}
</script>

<div class="mx-auto mt-16 max-w-md">
	<h1 class="mb-4 text-2xl font-semibold">Connect a desktop app</h1>
	{#if !valid || !codeChallenge}
		<p class="text-red-400" role="alert">
			This link is incomplete. Please start connecting again from the Rundenzeit app.
		</p>
	{:else if done === 'allowed'}
		<p class="text-zinc-300">Connecting… You can return to the app.</p>
	{:else if done === 'denied'}
		<p class="text-zinc-300">Cancelled.</p>
	{:else}
		<p class="mb-2 text-zinc-300">
			<strong>{deviceName}</strong> wants to sync with your account
			<strong>@{auth.me?.username}</strong>.
		</p>
		<p class="mb-6 text-sm text-zinc-400">
			It can upload your recorded sessions and download the ones from your other devices. It can't
			change your account. You can disconnect it any time on the Account page.
		</p>
		<p class="mb-6 text-xs text-zinc-500">
			Only allow this if you just clicked "Connect" in the Rundenzeit app on your computer.
		</p>
		{#if error}
			<p class="mb-4 text-sm text-red-400" role="alert">{error}</p>
		{/if}
		<div class="flex gap-2">
			<button
				type="button"
				class="rounded bg-zinc-100 px-4 py-2 text-sm font-medium text-zinc-900 hover:bg-white disabled:opacity-50"
				disabled={busy}
				onclick={allow}>Allow</button
			>
			<button
				type="button"
				class="rounded border border-zinc-700 px-4 py-2 text-sm hover:bg-zinc-800"
				onclick={deny}>Cancel</button
			>
		</div>
	{/if}
</div>
