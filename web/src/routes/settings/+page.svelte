<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { confirmAction } from '#lib/confirm.ts';
	import { api, type Settings } from '#lib/api/index.ts';
	import F1Capture from '#lib/components/F1Capture.svelte';
	import LmuCapture from '#lib/components/LmuCapture.svelte';
	import McpSetup from '#lib/components/McpSetup.svelte';
	import ProtonBridge from '#lib/components/ProtonBridge.svelte';
	import ServerSync from '#lib/components/ServerSync.svelte';
	import { formatBytes } from '#lib/format.ts';

	const client = api();
	const info = client.getInfo();
	let settings = $state<Settings | null>(null);
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

	let openError: string | null = $state(null);

	async function openDataDir() {
		openError = null;
		try {
			await client.openDataDir();
		} catch (e) {
			openError = e instanceof Error ? e.message : String(e);
		}
	}

	async function deleteRaw() {
		if (!(await confirmAction('Delete all kept raw recordings?'))) return;
		await client.deleteRawRecordings();
		await load();
	}

	type TabId = 'general' | 'sync' | 'games' | 'ai' | 'advanced' | 'about';

	// the active tab lives in the URL (?tab=…), so other pages can link to it
	const tabs = $derived(
		(
			[
				{ id: 'general', label: 'General', shown: client.capabilities.localSettings },
				{ id: 'sync', label: 'Sync', shown: client.capabilities.serverSync },
				{ id: 'games', label: 'Games', shown: client.capabilities.liveRecording },
				{ id: 'ai', label: 'AI', shown: !!settings?.mcpCommand },
				{ id: 'advanced', label: 'Advanced', shown: client.capabilities.localSettings },
				{ id: 'about', label: 'About', shown: true }
			] satisfies { id: TabId; label: string; shown: boolean }[]
		).filter((t) => t.shown)
	);
	const requested = $derived(page.url.searchParams.get('tab'));
	// some tabs only show up once the settings are loaded
	const loading = $derived(client.capabilities.localSettings && !settings && !error);
	const active = $derived<TabId | null>(
		tabs.find((t) => t.id === requested)?.id ?? (requested && loading ? null : tabs[0].id)
	);
</script>

<h1 class="mb-4 text-2xl font-semibold">
	{client.capabilities.localSettings ? 'Settings' : 'About'}
</h1>

{#if tabs.length > 1}
	<nav class="mb-4 flex max-w-2xl gap-1 border-b border-zinc-800 text-sm" aria-label="Settings">
		{#each tabs as t (t.id)}
			{@const current = t.id === active}
			<!-- eslint-disable svelte/no-navigation-without-resolve -- resolved path + query -->
			<a
				href={`${resolve('/settings')}?tab=${t.id}`}
				class="-mb-px border-b-2 px-3 py-1.5 hover:text-zinc-100"
				class:border-zinc-200={current}
				class:text-zinc-100={current}
				class:border-transparent={!current}
				class:text-zinc-400={!current}
				aria-current={current ? 'page' : undefined}
				data-sveltekit-replacestate
				data-sveltekit-noscroll>{t.label}</a
			>
			<!-- eslint-enable svelte/no-navigation-without-resolve -->
		{/each}
	</nav>
{/if}

{#if error}
	<p class="mb-4 max-w-2xl text-red-400" role="alert">{error}</p>
{/if}

{#if active === null}
	<p class="text-zinc-400">Loading…</p>
{:else if active === 'general'}
	<section class="mb-6 max-w-2xl rounded border border-zinc-800 p-4">
		<h2 class="mb-3 font-medium">Storage</h2>
		{#if settings}
			<dl class="grid grid-cols-[10rem_1fr] gap-y-1 text-sm">
				<dt class="text-zinc-400">Data directory</dt>
				<dd class="font-mono text-xs break-all">{settings.dataDir}</dd>
			</dl>
			<button
				type="button"
				class="mt-3 rounded border border-zinc-700 px-3 py-1 text-sm hover:border-zinc-500"
				onclick={openDataDir}
			>
				Open data directory
			</button>
			{#if openError}
				<p class="mt-2 text-sm text-red-400" role="alert">{openError}</p>
			{/if}
		{:else if loading}
			<p class="text-sm text-zinc-400">Loading…</p>
		{/if}
	</section>
{:else if active === 'sync'}
	<ServerSync />
{:else if active === 'games'}
	{#if settings}
		<!-- re-created after saving, so it shows the new status -->
		{#key settings}
			<F1Capture {settings} onsaved={() => setTimeout(load, 700)} />
			<LmuCapture {settings} onsaved={() => setTimeout(load, 700)} />
			{#if settings.bridgeAvailable}
				<ProtonBridge {settings} onsaved={() => setTimeout(load, 700)} />
			{/if}
		{/key}
	{:else if loading}
		<p class="text-zinc-400">Loading…</p>
	{/if}
{:else if active === 'ai'}
	{#if settings?.mcpCommand}
		<McpSetup mcp={settings.mcpCommand} />
	{/if}
{:else if active === 'advanced'}
	<section class="mb-6 max-w-2xl rounded border border-zinc-800 p-4">
		<h2 class="mb-1 font-medium">Raw recordings</h2>
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
		{:else if loading}
			<p class="text-sm text-zinc-400">Loading…</p>
		{/if}
	</section>
{:else if active === 'about'}
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
{/if}
