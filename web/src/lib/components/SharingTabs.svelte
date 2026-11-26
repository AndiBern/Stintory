<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';

	const tabs = [
		{ href: resolve('/shared'), label: 'Sessions and default' },
		{ href: resolve('/shared/tracks'), label: 'By track and car' }
	];
</script>

<nav class="mb-4 flex gap-1 border-b border-zinc-800 text-sm" aria-label="Sharing">
	{#each tabs as t (t.href)}
		{@const active = page.url.pathname === t.href}
		<a
			href={t.href}
			class="-mb-px border-b-2 px-3 py-1.5 hover:text-zinc-100"
			class:border-zinc-200={active}
			class:text-zinc-100={active}
			class:border-transparent={!active}
			class:text-zinc-400={!active}
			aria-current={active ? 'page' : undefined}
		>
			{t.label}
		</a>
	{/each}
</nav>
