<!-- SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { api, type SessionSummary } from '#lib/api/index.ts';
	import { app, runImport } from '#lib/app-state.svelte.ts';
	import { formatDateTime, formatGame, formatLapTime, formatSessionType } from '#lib/format.ts';

	const client = api();
	let sessions: Promise<SessionSummary[]> = $state(new Promise(() => {}));

	$effect(() => {
		void app.sessionsVersion; // refetch when sessions changed
		sessions = client.listSessions();
	});

	function open(id: string) {
		goto(resolve('/session/[id]', { id }));
	}
</script>

<div class="mb-4 flex items-center justify-between">
	<h1 class="text-2xl font-semibold">Sessions</h1>
	{#if client.capabilities.importFiles}
		<button
			type="button"
			class="rounded bg-zinc-100 px-3 py-1.5 text-sm font-medium text-zinc-900 hover:bg-white disabled:opacity-50"
			disabled={app.importing !== null}
			onclick={() => runImport(() => client.pickAndImportFiles())}
		>
			Import .ibt files…
		</button>
	{/if}
</div>

{#await sessions}
	<p class="text-zinc-400">Loading…</p>
{:then list}
	{#if list.length === 0}
		<div class="rounded border border-dashed border-zinc-700 p-8 text-center text-zinc-400">
			<p>No sessions yet.</p>
			<p class="mt-1 text-sm">
				Start driving – sessions are recorded automatically.
				{#if client.capabilities.importFiles}
					Or import iRacing <code>.ibt</code> files (drag them onto this window).
				{/if}
			</p>
		</div>
	{:else}
		<table class="w-full text-left text-sm">
			<thead class="text-zinc-400">
				<tr>
					<th class="py-2 font-normal">Date</th>
					<th class="font-normal">Track</th>
					<th class="font-normal">Car</th>
					<th class="font-normal">Type</th>
					<th class="text-right font-normal">Laps</th>
					<th class="text-right font-normal">Best</th>
				</tr>
			</thead>
			<tbody>
				{#each list as s (s.id)}
					<tr
						class="cursor-pointer border-t border-zinc-800 hover:bg-zinc-900"
						onclick={() => open(s.id)}
						onkeydown={(e) => e.key === 'Enter' && open(s.id)}
						tabindex="0"
					>
						<td class="py-2 whitespace-nowrap">{formatDateTime(s.startedAt)}</td>
						<td>
							<div>{s.trackName}</div>
							<div class="text-xs text-zinc-500">{formatGame(s.game)}</div>
						</td>
						<td>{s.carName}</td>
						<td>{formatSessionType(s.sessionType)}</td>
						<td class="text-right">{s.lapCount}</td>
						<td class="text-right font-mono">{formatLapTime(s.bestLapMs)}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
{:catch error}
	<p class="text-red-400" role="alert">Could not load sessions: {error.message}</p>
{/await}
