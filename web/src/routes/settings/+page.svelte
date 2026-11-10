<!-- SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { api } from '#lib/api/index.ts';

	const info = api().getInfo();
</script>

<h1 class="mb-4 text-2xl font-semibold">Settings</h1>

<section class="max-w-lg rounded border border-zinc-800 p-4">
	<h2 class="mb-2 font-medium">About</h2>
	{#await info}
		<p class="text-zinc-400">Loading…</p>
	{:then i}
		<dl class="grid grid-cols-[8rem_1fr] gap-y-1 text-sm">
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
