<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { untrack } from 'svelte';
	import { api, type LmuSource, type Settings } from '#lib/api/index.ts';
	import { errorText } from '#lib/app-state.svelte.ts';

	let { settings, onsaved }: { settings: Settings; onsaved: () => void } = $props();

	const client = api();
	let enabled = $state(untrack(() => settings.lmuCapture.enabled));
	let source = $state<LmuSource>(untrack(() => settings.lmuCapture.source ?? 'protonBridge'));
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

	async function save(next: { enabled: boolean; source: LmuSource }) {
		busy = true;
		error = null;
		try {
			await client.setLmuCapture(next);
			enabled = next.enabled;
			source = next.source;
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
			Stintory reads the game's own shared memory{#if settings.bridgeAvailable && source === 'sharedMemory'}&#32;from
				<code>/dev/shm/LMU_Data</code>, which another shared-memory bridge (e.g. simshmbridge or
				DataLink) has to create before the game starts{:else if settings.bridgeAvailable}&#32;through
				the Proton bridge: start the game with the launch option under
				<em>Proton bridge</em> below{/if}. In Le Mans Ultimate turn on
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
				onchange={(e) => save({ enabled: e.currentTarget.checked, source })}
			/>
			Record Le Mans Ultimate
		</label>
		{#if settings.bridgeAvailable}
			<fieldset class="mt-3 text-sm" disabled={busy || !enabled}>
				<legend class="mb-1 text-zinc-400">Read the game through</legend>
				<label class="flex items-center gap-2">
					<input
						type="radio"
						name="lmu-source"
						checked={source === 'protonBridge'}
						onchange={() => save({ enabled, source: 'protonBridge' })}
					/>
					Stintory's Proton bridge (recommended)
				</label>
				<label class="flex items-center gap-2">
					<input
						type="radio"
						name="lmu-source"
						checked={source === 'sharedMemory'}
						onchange={() => save({ enabled, source: 'sharedMemory' })}
					/>
					Another shared-memory bridge (<code>/dev/shm/LMU_Data</code>)
				</label>
			</fieldset>
		{/if}
		{#if error}<p class="mt-2 text-sm text-red-400" role="alert">{error}</p>{/if}
	{:else}
		<p class="text-sm text-zinc-400">
			{settings.lmuStatus ?? 'Le Mans Ultimate can only be recorded on Windows.'} Sessions recorded on
			a Windows PC appear here through sync or a <code>.stintory</code> import.
		</p>
	{/if}
</section>
