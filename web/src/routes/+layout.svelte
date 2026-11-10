<!-- SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import './layout.css';
	import favicon from '#lib/assets/favicon.svg';
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import { app, connect } from '#lib/app-state.svelte.ts';
	import { formatLapTime, formatGame } from '#lib/format.ts';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();

	const nav = [
		{ href: resolve('/'), label: 'Sessions' },
		{ href: resolve('/settings'), label: 'Settings' }
	];

	function isActive(href: string): boolean {
		return href === resolve('/')
			? page.url.pathname === href ||
					page.url.pathname.startsWith('/session/') ||
					page.url.pathname.startsWith('/compare/')
			: page.url.pathname.startsWith(href);
	}

	$effect(() => connect());
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
	<title>Rundenzeit</title>
</svelte:head>

<div class="flex h-screen bg-zinc-950 text-zinc-100">
	<nav class="flex w-52 shrink-0 flex-col gap-1 border-r border-zinc-800 p-3" aria-label="Main">
		<div class="mb-4 px-2 text-lg font-semibold tracking-tight">Rundenzeit</div>
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
		</div>
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
		{@render children()}
	</main>
</div>
