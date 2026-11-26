<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { page } from '$app/state';
	import { accountApi, type OAuthAuthorizeInfo } from '#lib/api/index.ts';
	import { auth, errorText } from '#lib/app-state.svelte.ts';

	// An AI assistant (MCP client) asks for read access through OAuth
	// (docs/SERVER.md): the server checks the request, the user decides.
	const account = accountApi();
	const KEYS = [
		'response_type',
		'client_id',
		'redirect_uri',
		'code_challenge',
		'code_challenge_method',
		'state',
		'scope',
		'resource'
	];
	const params: Record<string, string> = {};
	for (const k of KEYS) {
		const v = page.url.searchParams.get(k);
		if (v !== null) params[k] = v;
	}

	let info = $state<OAuthAuthorizeInfo | null>(null);
	let error = $state<string | null>(null);
	let busy = $state(false);
	let leaving = $state(false);

	function go(to: string) {
		leaving = true;
		window.location.href = to;
	}

	$effect(() => {
		if (!account || !auth.me) return;
		account
			.oauthAuthorizeInfo(page.url.search)
			.then((i) => {
				if (i.redirectTo) go(i.redirectTo);
				else info = i;
			})
			.catch((e) => (error = errorText(e)));
	});

	async function decide(allow: boolean) {
		if (!account) return;
		busy = true;
		error = null;
		try {
			go((await account.oauthApprove(params, allow)).redirectTo);
		} catch (e) {
			error = errorText(e);
			leaving = false;
		} finally {
			busy = false;
		}
	}
</script>

<div class="mx-auto mt-16 max-w-md">
	<h1 class="mb-4 text-2xl font-semibold">Connect an AI assistant</h1>
	{#if leaving}
		<p class="text-zinc-300">Returning to the assistant…</p>
	{:else if error && !info}
		<p class="text-red-400" role="alert">{error}</p>
		<p class="mt-2 text-sm text-zinc-400">Please start connecting again from the assistant.</p>
	{:else if !info}
		<p class="text-zinc-400">Checking the request…</p>
	{:else}
		<p class="mb-2 text-zinc-300">
			<strong>{info.clientName}</strong> wants to read the laps of your account
			<strong>@{auth.me?.username}</strong>.
		</p>
		<dl class="mb-4 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-sm">
			<dt class="text-zinc-500">Client</dt>
			<dd class="break-all text-zinc-300">
				{#if info.clientHost}
					{info.clientHost}
				{:else}
					registered by the app itself <span class="text-zinc-500">(not verified)</span>
				{/if}
			</dd>
			<dt class="text-zinc-500">Returns to</dt>
			<dd class="break-all text-zinc-300">
				{info.redirectHost}{#if info.localRedirect}
					<span class="ml-1 text-zinc-500">(a program on this computer)</span>{/if}
			</dd>
		</dl>
		<p class="mb-4 text-sm text-zinc-400">
			It can read your sessions, laps and the corner analysis, and laps others share with you. It
			can't change, delete or share anything. You can remove it any time on the Account page (AI
			assistants).
		</p>
		{#if info.localRedirect}
			<p class="mb-4 text-xs text-amber-300/90">
				Only allow this if you just started connecting in an assistant on this computer.
			</p>
		{:else}
			<p class="mb-4 text-xs text-zinc-500">
				Only allow this if you just started connecting from {info.redirectHost}.
			</p>
		{/if}
		{#if error}
			<p class="mb-4 text-sm text-red-400" role="alert">{error}</p>
		{/if}
		<div class="flex gap-2">
			<button
				type="button"
				class="rounded bg-zinc-100 px-4 py-2 text-sm font-medium text-zinc-900 hover:bg-white disabled:opacity-50"
				disabled={busy}
				onclick={() => decide(true)}
			>
				Allow
			</button>
			<button
				type="button"
				class="rounded border border-zinc-700 px-4 py-2 text-sm hover:bg-zinc-800 disabled:opacity-50"
				disabled={busy}
				onclick={() => decide(false)}
			>
				Deny
			</button>
		</div>
	{/if}
</div>
