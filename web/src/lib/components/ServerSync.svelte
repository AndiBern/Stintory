<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { confirmAction } from '#lib/confirm.ts';
	import { api } from '#lib/api/index.ts';
	import { app, errorText } from '#lib/app-state.svelte.ts';
	import { formatAgo } from '#lib/format.ts';

	const client = api();

	let serverUrl = $state('');
	let token = $state('');
	let useToken = $state(false);
	let busy = $state(false);
	let error = $state<string | null>(null);

	const s = $derived(app.sync);

	let appVersion = $state<string | null>(null);
	client
		.getInfo()
		.then((i) => (appVersion = i.version))
		.catch(() => {});

	async function act(action: () => Promise<unknown>) {
		busy = true;
		error = null;
		try {
			await action();
		} catch (e) {
			error = errorText(e);
		} finally {
			busy = false;
		}
	}

	function connect(event: SubmitEvent) {
		event.preventDefault();
		act(async () => {
			app.sync = useToken
				? await client.syncConnectToken(serverUrl, token)
				: await client.syncConnect(serverUrl);
			token = '';
		});
	}

	async function disconnect() {
		if (
			!(await confirmAction(
				'Disconnect from the server? Your data on this computer and on the server stays; this computer just stops syncing.'
			))
		)
			return;
		act(async () => {
			app.sync = await client.syncDisconnect();
		});
	}

	const input =
		'w-full rounded border border-zinc-700 bg-zinc-900 px-3 py-1.5 text-sm focus:border-zinc-400 focus:outline-none';
	const button =
		'rounded bg-zinc-100 px-3 py-1.5 text-sm font-medium text-zinc-900 hover:bg-white disabled:opacity-50';
	const secondary = 'rounded border border-zinc-700 px-3 py-1.5 text-sm hover:bg-zinc-800';
</script>

<section class="mb-6 max-w-2xl rounded border border-zinc-800 p-4">
	<h2 class="mb-1 font-medium">Server sync</h2>
	<p class="mb-3 text-sm text-zinc-400">
		Sync your sessions with your own Stintory server, to see them on your other computers and in the
		browser.
	</p>

	{#if !s}
		<p class="text-sm text-zinc-400">Loading…</p>
	{:else if s.state === 'connecting'}
		<p class="mb-3 text-sm text-zinc-300">
			Your browser opened the server's login page. Log in there and allow this computer to sync.
		</p>
		<button type="button" class={secondary} onclick={() => client.syncCancelConnect()}>
			Cancel
		</button>
	{:else if s.state === 'disconnected'}
		<form class="flex flex-col gap-3" onsubmit={connect}>
			<label class="flex flex-col gap-1 text-sm">
				<span class="text-zinc-400">Server address</span>
				<input
					class={input}
					bind:value={serverUrl}
					placeholder="https://laps.example.com"
					inputmode="url"
					required
				/>
			</label>
			{#if useToken}
				<label class="flex flex-col gap-1 text-sm">
					<span class="text-zinc-400">
						Device token (Account page on the server → Devices → Create token)
					</span>
					<input class="{input} font-mono" bind:value={token} placeholder="syd_…" required />
				</label>
			{/if}
			<div class="flex items-center gap-3">
				<button type="submit" class={button} disabled={busy}>
					{useToken ? 'Connect' : 'Connect in browser…'}
				</button>
				<button
					type="button"
					class="text-xs text-zinc-400 hover:text-zinc-200"
					onclick={() => (useToken = !useToken)}
				>
					{useToken ? 'Log in through the browser instead' : 'Use a device token instead'}
				</button>
			</div>
		</form>
	{:else}
		<dl class="mb-3 grid grid-cols-[9rem_1fr] gap-y-1 text-sm">
			<dt class="text-zinc-400">Server</dt>
			<dd class="break-all">{s.serverUrl}</dd>
			<dt class="text-zinc-400">Server version</dt>
			<dd>
				{s.serverVersion ?? 'not known yet'}
				{#if s.updateNeeded && appVersion}
					<span class="text-zinc-500">· this app: {appVersion}</span>
				{/if}
			</dd>
			<dt class="text-zinc-400">Account</dt>
			<dd>@{s.username}</dd>
			<dt class="text-zinc-400">This computer</dt>
			<dd>{s.deviceName}</dd>
			<dt class="text-zinc-400">Last sync</dt>
			<dd>
				{#if s.state === 'syncing'}
					syncing…
				{:else}
					{formatAgo(s.lastSyncAt)}
					{#if s.lastSyncAt && (s.lastPushed || s.lastPulled)}
						<span class="text-zinc-500">· {s.lastPushed} sent, {s.lastPulled} received</span>
					{/if}
				{/if}
			</dd>
		</dl>
		{#if s.updateNeeded}
			<div
				class="mb-3 rounded border border-amber-400/40 bg-amber-400/10 p-3 text-sm text-amber-200"
				role="alert"
			>
				<p class="font-medium">
					{#if s.updateNeeded === 'app'}
						Update the app to sync
					{:else if s.updateNeeded === 'server'}
						Update the server to sync
					{:else}
						Update the app or the server to sync
					{/if}
				</p>
				<p class="mt-1 text-amber-200/80">
					The app and the server only sync when both run the same version. Syncing resumes once they
					match.
				</p>
			</div>
		{/if}
		{#if !s.tokenInKeychain}
			<p class="mb-3 text-xs text-amber-300/80">
				No system keychain was found, so the login is kept in the app's data folder.
			</p>
		{/if}
		<label class="mb-3 flex items-center gap-2 text-sm">
			<input
				type="checkbox"
				checked={s.autoSync}
				onchange={(e) =>
					act(async () => (app.sync = await client.syncSetAuto(e.currentTarget.checked)))}
			/>
			Sync automatically (after recording and every few minutes)
		</label>
		<div class="flex gap-2">
			<button
				type="button"
				class={button}
				disabled={busy || s.state === 'syncing'}
				onclick={() => act(async () => (app.sync = await client.syncNow()))}
			>
				Sync now
			</button>
			<button type="button" class={secondary} disabled={busy} onclick={disconnect}>
				Disconnect
			</button>
		</div>
	{/if}

	{#if error ?? (s?.updateNeeded ? null : s?.lastError)}
		<p class="mt-3 text-sm text-red-400" role="alert">{error ?? s?.lastError}</p>
	{/if}
</section>
