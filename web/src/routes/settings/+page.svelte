<!-- SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { api, type Settings } from '#lib/api/index.ts';
	import ServerSync from '#lib/components/ServerSync.svelte';
	import { formatBytes } from '#lib/format.ts';

	const client = api();
	const info = client.getInfo();
	let settings: Settings | null = $state(null);
	let error: string | null = $state(null);

	async function load() {
		try {
			settings = await client.getSettings();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	if (client.capabilities.localSettings) load();

	async function setKeepRaw(keep: boolean) {
		await client.setKeepRawRecordings(keep);
		await load();
	}

	async function deleteRaw() {
		if (!confirm('Delete all kept raw recordings?')) return;
		await client.deleteRawRecordings();
		await load();
	}
</script>

<h1 class="mb-4 text-2xl font-semibold">
	{client.capabilities.localSettings ? 'Settings' : 'About'}
</h1>

{#if client.capabilities.serverSync}
	<ServerSync />
{/if}

{#if client.capabilities.localSettings}
	<section class="mb-6 max-w-2xl rounded border border-zinc-800 p-4">
		<h2 class="mb-3 font-medium">Storage</h2>
		{#if error}
			<p class="text-red-400" role="alert">{error}</p>
		{:else if settings}
			<dl class="grid grid-cols-[10rem_1fr] gap-y-1 text-sm">
				<dt class="text-zinc-400">Data directory</dt>
				<dd class="font-mono text-xs break-all">{settings.dataDir}</dd>
			</dl>
		{/if}
	</section>

	<section class="mb-6 max-w-2xl rounded border border-zinc-800 p-4">
		<h2 class="mb-1 font-medium">Advanced / Debug</h2>
		<p class="mb-3 text-sm text-zinc-400">
			Raw game data is written to disk while recording and deleted once it is processed. Keep it
			only for debugging or bug reports. Raw files are never synced or exported.
		</p>
		{#if settings}
			<label class="flex items-center gap-2 text-sm">
				<input
					type="checkbox"
					checked={settings.keepRawRecordings}
					onchange={(e) => setKeepRaw(e.currentTarget.checked)}
				/>
				Keep raw recordings
			</label>
			<p class="mt-2 text-sm text-zinc-400">
				Raw recordings on disk: {formatBytes(settings.rawStorageBytes)}
				{#if settings.failedRecordings > 0}
					· <span class="text-amber-400">{settings.failedRecordings} could not be processed</span>
				{/if}
			</p>
			{#if settings.rawStorageBytes > 0}
				<button
					type="button"
					class="mt-2 rounded border border-zinc-700 px-3 py-1 text-sm hover:border-red-700 hover:text-red-300"
					onclick={deleteRaw}
				>
					Delete raw recordings
				</button>
			{/if}
		{/if}
	</section>
{/if}

<section class="max-w-2xl rounded border border-zinc-800 p-4">
	<h2 class="mb-2 font-medium">About</h2>
	{#await info}
		<p class="text-zinc-400">Loading…</p>
	{:then i}
		<dl class="grid grid-cols-[10rem_1fr] gap-y-1 text-sm">
			<dt class="text-zinc-400">Application</dt>
			<dd>{i.name}</dd>
			<dt class="text-zinc-400">Version</dt>
			<dd>{i.version}</dd>
			<dt class="text-zinc-400">Backend</dt>
			<dd>{i.backend}</dd>
		</dl>
	{:catch error}
		<p class="text-red-400" role="alert">Backend not reachable: {error.message}</p>
	{/await}
</section>
