<!-- SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import './layout.css';
	import favicon from '#lib/assets/favicon.svg';
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();

	const nav = [
		{ href: resolve('/'), label: 'Sessions' },
		{ href: resolve('/settings'), label: 'Settings' }
	];

	function isActive(href: string): boolean {
		return href === resolve('/') ? page.url.pathname === href : page.url.pathname.startsWith(href);
	}
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
	<title>Rundenzeit</title>
</svelte:head>

<div class="flex h-screen bg-zinc-950 text-zinc-100">
	<nav class="flex w-48 shrink-0 flex-col gap-1 border-r border-zinc-800 p-3" aria-label="Main">
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
	</nav>
	<main class="flex-1 overflow-auto p-6">
		{@render children()}
	</main>
</div>
