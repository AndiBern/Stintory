<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { untrack } from 'svelte';
	import { api, type Settings } from '#lib/api/index.ts';
	import { errorText } from '#lib/app-state.svelte.ts';

	let { settings, onsaved }: { settings: Settings; onsaved: () => void } = $props();

	const client = api();
	let enabled = $state(untrack(() => settings.lmuCapture.enabled));
	// the game's state changes while the page is open: refresh it
	let status = $state(untrack(() => settings.lmuStatus));
	let hint = $state(untrack(() => settings.lmuHint));
	let error = $state<string | null>(null);
	let busy = $state(false);

	$effect(() => {
		if (!settings.lmuAvailable) return;
		const timer = setInterval(async () => {
			try {
				const s = await client.getSettings();
				status = s.lmuStatus;
				hint = s.lmuHint;
			} catch {
				// keep the last status
			}
		}, 2000);
		return () => clearInterval(timer);
	});

	async function toggle(value: boolean) {
		busy = true;
		error = null;
		try {
			await client.setLmuCapture({ enabled: value });
			enabled = value;
			onsaved();
		} catch (e) {
			error = errorText(e);
		} finally {
			busy = false;
		}
	}
</script>

<section class="mb-6 max-w-2xl rounded border border-zinc-800 p-4">
	<h2 class="mb-1 font-medium">Le Mans Ultimate</h2>
	{#if settings.lmuAvailable}
		<p class="mb-3 text-sm text-zinc-400">
			Stintory reads the game's own shared memory. In Le Mans Ultimate turn on
			<em>Settings → Gameplay → Enable Plugins</em> once and restart the game; no plugin needs to be installed.
			Recording starts when you drive out of the garage.
		</p>
		<p class="mb-3 text-sm" role="status">
			{#if !enabled}
				<span class="text-zinc-400">Off.</span>
			{:else if status}
				<span class="text-zinc-300">{status}</span>
			{:else}
				<span class="text-zinc-400">Starting…</span>
			{/if}
			{#if enabled && hint}
				<span class="mt-1 block text-amber-400" role="alert">{hint}</span>
			{/if}
		</p>
		<label class="flex items-center gap-2 text-sm">
			<input
				type="checkbox"
				checked={enabled}
				disabled={busy}
				onchange={(e) => toggle(e.currentTarget.checked)}
			/>
			Record Le Mans Ultimate
		</label>
		{#if error}<p class="mt-2 text-sm text-red-400" role="alert">{error}</p>{/if}
	{:else}
		<p class="text-sm text-zinc-400">
			{settings.lmuStatus ?? 'Le Mans Ultimate can only be recorded on Windows.'} Sessions recorded on
			a Windows PC appear here through sync or a <code>.stintory</code> import.
		</p>
	{/if}
</section>
