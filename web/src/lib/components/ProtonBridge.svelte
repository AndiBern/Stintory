<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { untrack } from 'svelte';
	import { api, type ProtonBridgeSettings, type Settings } from '#lib/api/index.ts';
	import { errorText } from '#lib/app-state.svelte.ts';

	let { settings, onsaved }: { settings: Settings; onsaved: () => void } = $props();

	const client = api();
	// the form starts from the saved settings; the page re-creates it after saving
	let form = $state<ProtonBridgeSettings>(untrack(() => $state.snapshot(settings.protonBridge)));
	// the connection changes while the page is open: refresh it
	let status = $state(untrack(() => settings.bridgeStatus));
	let bridgeError = $state(untrack(() => settings.bridgeError));
	let error = $state<string | null>(null);
	let saved = $state(false);
	let copied = $state(false);
	let busy = $state(false);

	$effect(() => {
		const timer = setInterval(async () => {
			try {
				const s = await client.getSettings();
				status = s.bridgeStatus;
				bridgeError = s.bridgeError;
			} catch {
				// keep the last status
			}
		}, 2000);
		return () => clearInterval(timer);
	});

	async function copy() {
		if (!settings.bridgeLaunchOption) return;
		try {
			await navigator.clipboard.writeText(settings.bridgeLaunchOption);
			copied = true;
			setTimeout(() => (copied = false), 2000);
		} catch (e) {
			error = errorText(e);
		}
	}

	async function save(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = null;
		saved = false;
		try {
			await client.setProtonBridge({ ...form, port: Number(form.port) });
			saved = true;
			onsaved();
		} catch (e) {
			error = errorText(e);
		} finally {
			busy = false;
		}
	}
</script>

<section class="mb-6 max-w-2xl rounded border border-zinc-800 p-4">
	<h2 class="mb-1 font-medium">Proton bridge</h2>
	<p class="mb-3 text-sm text-zinc-400">
		Games that run in Proton keep their telemetry inside Wine, where Stintory can't see it. The
		small helper <code>sy-bridge.exe</code> runs next to the game in its Proton prefix and passes
		your car's data to Stintory on this PC. For Le Mans Ultimate, add this to the game's
		<em>Properties → General → Launch Options</em> in Steam:
	</p>
	{#if settings.bridgeLaunchOption}
		<div class="mb-1 flex items-start gap-2">
			<code
				class="block flex-1 rounded border border-zinc-700 bg-zinc-950 px-2 py-1 font-mono text-xs break-all select-all"
			>
				{settings.bridgeLaunchOption}
			</code>
			<button
				type="button"
				class="rounded border border-zinc-700 px-3 py-1 text-sm hover:border-zinc-500"
				onclick={copy}
			>
				{copied ? 'Copied' : 'Copy'}
			</button>
		</div>
		<p class="mb-3 text-xs text-zinc-500">
			Keep other launch options before <code>%command%</code>. Stintory doesn't have to run before
			the game; the bridge waits for it and ends with the game.
		</p>
	{/if}
	<p class="mb-3 text-sm" role="status">
		{#if !settings.protonBridge.enabled}
			<span class="text-zinc-400">Off.</span>
		{:else if status}
			<span class="text-zinc-300">{status}</span>
		{:else}
			<span class="text-zinc-400">Starting…</span>
		{/if}
		{#if bridgeError}
			<span class="mt-1 block text-amber-400" role="alert">{bridgeError}</span>
		{/if}
	</p>
	<form class="flex flex-col gap-3 text-sm" onsubmit={save}>
		<label class="flex items-center gap-2">
			<input type="checkbox" bind:checked={form.enabled} />
			Record games through the Proton bridge
		</label>
		<label class="flex items-center gap-2">
			<span class="w-40 text-zinc-400">TCP port</span>
			<input
				type="number"
				min="1024"
				max="65535"
				class="w-28 rounded border border-zinc-700 bg-zinc-950 px-2 py-1"
				bind:value={form.port}
			/>
			<span class="text-xs text-zinc-500">on 127.0.0.1; the launch option above includes it</span>
		</label>
		<div class="flex items-center gap-3">
			<button
				type="submit"
				class="rounded bg-zinc-100 px-3 py-1.5 font-medium text-zinc-900 hover:bg-white disabled:opacity-50"
				disabled={busy}
			>
				Save
			</button>
			{#if error}<span class="text-red-400" role="alert">{error}</span>{/if}
			{#if saved}<span class="text-zinc-300" role="status">
					Saved. Applies once no game is being recorded through the bridge.
				</span>{/if}
		</div>
	</form>
</section>
