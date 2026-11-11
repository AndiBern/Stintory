<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import './layout.css';
	import favicon from '#lib/assets/favicon.svg';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import { accountApi, api } from '#lib/api/index.ts';
	import { app, auth, connect, connectAuth, sharingAvailable } from '#lib/app-state.svelte.ts';
	import { formatAgo, formatLapTime, formatGame } from '#lib/format.ts';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();

	const client = api();
	const account = accountApi();

	/** Pages that work without login. */
	const publicPaths: string[] = [resolve('/login')];
	const isPublic = $derived(publicPaths.includes(page.url.pathname));

	const nav = $derived([
		{ href: resolve('/'), label: 'Sessions' },
		...(sharingAvailable()
			? [
					{ href: resolve('/shared'), label: 'Sharing' },
					{ href: resolve('/teams'), label: 'Teams' }
				]
			: []),
		...(account && auth.me ? [{ href: resolve('/account'), label: 'Account' }] : []),
		...(auth.me?.isAdmin ? [{ href: resolve('/admin'), label: 'Admin' }] : []),
		{ href: resolve('/settings'), label: client.capabilities.localSettings ? 'Settings' : 'About' }
	]);

	function isActive(href: string): boolean {
		// sessions and laps someone shared belong to "Sharing"
		const shared = page.url.searchParams.has('owner');
		const detail =
			page.url.pathname.startsWith('/session/') || page.url.pathname.startsWith('/compare/');
		if (href === resolve('/')) return page.url.pathname === href || (detail && !shared);
		if (href === resolve('/shared'))
			return page.url.pathname.startsWith(href) || (detail && shared);
		return page.url.pathname.startsWith(href);
	}

	$effect(() => connect());
	$effect(() => connectAuth());

	// server: log in first; restricted accounts go to the account page
	$effect(() => {
		if (!account || !auth.ready || !auth.reachable) return;
		const path = page.url.pathname;
		if (!auth.me) {
			// come back here after logging in (e.g. the desktop app's device login)
			if (!isPublic && !loggingOut) {
				const next = encodeURIComponent(path + page.url.search);
				// eslint-disable-next-line svelte/no-navigation-without-resolve -- resolved path + query
				goto(`${resolve('/login')}?next=${next}`);
			}
		} else if (
			(auth.me.mustChangePassword || auth.me.twoFactorRequired) &&
			path !== resolve('/account')
		) {
			goto(resolve('/account'));
		}
	});

	// on the server, pages render only once it's clear who is logged in
	const showPage = $derived(
		!account || (auth.ready && (!auth.reachable || auth.me !== null || isPublic))
	);

	/** An explicit logout goes to the plain login page (no "come back"). */
	let loggingOut = false;

	async function logout() {
		loggingOut = true;
		await account?.logout().catch(() => {});
		auth.me = null;
		await goto(resolve('/login'));
		loggingOut = false;
	}
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
	<title>Stintory</title>
</svelte:head>

<div class="flex h-screen bg-zinc-950 text-zinc-100">
	<nav class="flex w-52 shrink-0 flex-col gap-1 border-r border-zinc-800 p-3" aria-label="Main">
		<div class="mb-4 px-2 text-lg font-semibold tracking-tight">Stintory</div>
		{#if !account || auth.me || !auth.reachable}
			{#each nav as item (item.href)}
				<a
					href={item.href}
					class="rounded px-2 py-1.5 text-sm hover:bg-zinc-800"
					class:bg-zinc-800={isActive(item.href)}
					aria-current={isActive(item.href) ? 'page' : undefined}
				>
					{item.label}
				</a>
			{/each}
		{/if}

		{#if account}
			{#if auth.me}
				<div class="mt-auto rounded border border-zinc-800 p-2 text-xs">
					<div class="truncate text-zinc-300">{auth.me.displayName}</div>
					<div class="truncate text-zinc-500">@{auth.me.username}</div>
					<button type="button" class="mt-2 text-zinc-400 hover:text-zinc-100" onclick={logout}>
						Log out
					</button>
				</div>
			{/if}
		{:else}
			<div
				class="mt-auto rounded border border-zinc-800 p-2 text-xs"
				role="status"
				aria-live="polite"
			>
				{#if app.recorder?.state === 'recording'}
					<div class="flex items-center gap-1.5 font-medium text-red-400">
						<span class="inline-block h-2 w-2 animate-pulse rounded-full bg-red-500"></span>
						Recording {app.recorder.game ? formatGame(app.recorder.game) : ''}
					</div>
					<div class="mt-1 truncate text-zinc-300">{app.recorder.trackName}</div>
					<div class="text-zinc-400">
						{app.recorder.laps} laps · last {formatLapTime(app.recorder.lastLapMs)}
					</div>
				{:else if app.recorder?.state === 'waiting'}
					<div class="text-zinc-400">Waiting for a game…</div>
				{:else if app.recorder?.state === 'unsupported'}
					<div class="text-zinc-500">Live recording: Windows only</div>
				{/if}
				{#if app.sync && app.sync.state !== 'disconnected'}
					<a
						href={resolve('/settings')}
						class="mt-2 block border-t border-zinc-800 pt-2 text-zinc-400 hover:text-zinc-200"
						title={app.sync.lastError ?? app.sync.serverUrl ?? ''}
					>
						{#if app.sync.state === 'syncing'}
							Syncing…
						{:else if app.sync.state === 'connecting'}
							Connecting…
						{:else if app.sync.lastError}
							<span class="text-amber-400">Sync failed</span>
						{:else}
							Synced {formatAgo(app.sync.lastSyncAt)}
						{/if}
					</a>
				{/if}
			</div>
		{/if}
	</nav>
	<main class="flex-1 overflow-auto p-6">
		{#if app.importing}
			<div class="mb-4 rounded border border-zinc-700 bg-zinc-900 p-3 text-sm" role="status">
				Importing… {Math.round(app.importing.fraction * 100)}% · {app.importing.laps} laps
				<div class="mt-2 h-1.5 overflow-hidden rounded bg-zinc-800">
					<div
						class="h-full bg-zinc-300 transition-[width]"
						style:width="{app.importing.fraction * 100}%"
					></div>
				</div>
			</div>
		{:else if app.notice}
			<div
				class="mb-4 flex items-start justify-between rounded border p-3 text-sm {app.notice.tone ===
				'error'
					? 'border-red-800 bg-red-950 text-red-200'
					: 'border-emerald-800 bg-emerald-950 text-emerald-200'}"
				role="status"
			>
				<span class="break-all">{app.notice.text}</span>
				<button
					type="button"
					class="ml-4 text-xs opacity-70 hover:opacity-100"
					onclick={() => (app.notice = null)}>Dismiss</button
				>
			</div>
		{/if}
		{#if showPage}
			{@render children()}
		{/if}
	</main>
</div>
