<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { api, type SessionSummary } from '#lib/api/index.ts';
	import { app, runImport } from '#lib/app-state.svelte.ts';
	import ExportDialog, { type ExportScope } from '#lib/components/ExportDialog.svelte';
	import {
		fileDate,
		fileName,
		formatDateTime,
		formatGame,
		formatLapTime,
		formatOrigin,
		formatSessionType
	} from '#lib/format.ts';

	const client = api();
	let sessions: Promise<SessionSummary[]> = $state(new Promise(() => {}));
	let list: SessionSummary[] = $state([]);
	let selected = $state<string[]>([]);
	let exporting = $state(false);

	$effect(() => {
		void app.sessionsVersion; // refetch when sessions changed
		const pending = client.listSessions();
		sessions = pending;
		pending
			.then((l) => {
				list = l;
				selected = selected.filter((id) => l.some((s) => s.id === id));
			})
			.catch(() => {}); // shown by the {#await} block
	});

	function open(id: string) {
		goto(resolve('/session/[id]', { id }));
	}

	function toggle(id: string) {
		selected = selected.includes(id) ? selected.filter((s) => s !== id) : [...selected, id];
	}

	const exportScopes: ExportScope[] = $derived.by(() => {
		const chosen = list.filter((s) => selected.includes(s.id));
		const first = chosen[0];
		if (!first) return [];
		const laps = chosen.reduce((n, s) => n + s.lapCount, 0);
		return [
			{
				label:
					chosen.length === 1
						? `${first.trackName}, ${first.carName} (${laps} laps)`
						: `${chosen.length} sessions (${laps} laps)`,
				selection: { sessionIds: chosen.map((s) => s.id) },
				fileName:
					chosen.length === 1
						? fileName(first.trackName, first.carName, fileDate(first.startedAt))
						: fileName('Stintory', `${chosen.length} sessions`, fileDate(new Date().toISOString()))
			}
		];
	});
</script>

<div class="mb-4 flex items-center justify-between gap-2">
	<h1 class="text-2xl font-semibold">Sessions</h1>
	<div class="flex gap-2">
		{#if client.capabilities.exportFiles}
			<button
				type="button"
				class="rounded border border-zinc-700 px-3 py-1.5 text-sm hover:bg-zinc-800 disabled:opacity-40"
				disabled={selected.length === 0}
				onclick={() => (exporting = true)}
			>
				{selected.length > 0 ? `Export ${selected.length}…` : 'Export…'}
			</button>
		{/if}
		{#if client.capabilities.importFiles}
			<button
				type="button"
				class="rounded bg-zinc-100 px-3 py-1.5 text-sm font-medium text-zinc-900 hover:bg-white disabled:opacity-50"
				disabled={app.importing !== null}
				onclick={() => runImport(() => client.pickAndImportFiles())}
			>
				Import…
			</button>
		{/if}
	</div>
</div>

{#if client.capabilities.exportFiles}
	<ExportDialog bind:open={exporting} scopes={exportScopes} />
{/if}

{#await sessions}
	<p class="text-zinc-400">Loading…</p>
{:then}
	{#if list.length === 0}
		<div class="rounded border border-dashed border-zinc-700 p-8 text-center text-zinc-400">
			<p>No sessions yet.</p>
			<p class="mt-1 text-sm">
				{#if client.capabilities.liveRecording}
					Start driving – sessions are recorded automatically. Or import Stintory <code
						>.stintory</code
					>
					or iRacing <code>.ibt</code> files (drag them onto this window).
				{:else}
					Sync from the desktop app, or upload <code>.stintory</code> files exported there with "Import…".
				{/if}
			</p>
		</div>
	{:else}
		<table class="w-full text-left text-sm">
			<thead class="text-zinc-400">
				<tr>
					{#if client.capabilities.exportFiles}
						<th class="w-8 py-2"><span class="sr-only">Select</span></th>
					{/if}
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
						{#if client.capabilities.exportFiles}
							<td class="py-2">
								<input
									type="checkbox"
									aria-label="Select session {s.trackName} {formatDateTime(s.startedAt)}"
									checked={selected.includes(s.id)}
									onclick={(e) => e.stopPropagation()}
									onkeydown={(e) => e.stopPropagation()}
									onchange={() => toggle(s.id)}
								/>
							</td>
						{/if}
						<td class="py-2 whitespace-nowrap">{formatDateTime(s.startedAt)}</td>
						<td>
							<div>{s.trackName}</div>
							<div class="text-xs text-zinc-500">
								{formatGame(s.game)}
								{#if formatOrigin(s.origin)}
									<span class="ml-1 rounded bg-sky-950 px-1.5 text-sky-300"
										>{formatOrigin(s.origin)}</span
									>
								{/if}
							</div>
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
