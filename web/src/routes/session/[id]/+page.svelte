<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { api, type LapSummary, type SessionDetail } from '#lib/api/index.ts';
	import { app } from '#lib/app-state.svelte.ts';
	import ExportDialog, { type ExportScope } from '#lib/components/ExportDialog.svelte';
	import {
		fileDate,
		fileName,
		formatDateTime,
		formatDelta,
		formatGame,
		formatLapTime,
		formatOrigin,
		formatSessionType
	} from '#lib/format.ts';

	const MAX_COMPARE = 8;
	const client = api();
	const id = $derived(page.params.id ?? '');

	let detail = $state<SessionDetail | null>(null);
	let error = $state<string | null>(null);
	let selected = $state<string[]>([]);
	let showAll = $state(false);
	let exporting = $state(false);

	$effect(() => {
		void app.sessionsVersion;
		const sessionId = id;
		error = null;
		client
			.getSession(sessionId)
			.then((d) => {
				const keepSelection = detail?.summary.id === d.summary.id;
				detail = d;
				if (!keepSelection) selected = bestLap(d) ? [bestLap(d)!.id] : [];
			})
			.catch((e) => (error = e.message));
	});

	function isClean(l: LapSummary): boolean {
		return l.valid && !l.outLap && !l.inLap && !l.incomplete && l.lapTimeMs !== null;
	}

	function bestLap(d: SessionDetail): LapSummary | undefined {
		return d.laps
			.filter(isClean)
			.reduce<LapSummary | undefined>(
				(b, l) => (!b || l.lapTimeMs! < b.lapTimeMs! ? l : b),
				undefined
			);
	}

	const best = $derived(detail ? bestLap(detail) : undefined);
	const visibleLaps = $derived(detail ? detail.laps.filter((l) => showAll || !l.incomplete) : []);
	const hiddenCount = $derived(detail ? detail.laps.length - visibleLaps.length : 0);

	function toggle(lapId: string) {
		if (selected.includes(lapId)) selected = selected.filter((s) => s !== lapId);
		else if (selected.length < MAX_COMPARE) selected = [...selected, lapId];
	}

	function compare() {
		if (!detail) return;
		// reference = fastest selected lap
		const time = (lapId: string) =>
			detail!.laps.find((l) => l.id === lapId)?.lapTimeMs ?? Number.MAX_SAFE_INTEGER;
		const ordered = [...selected].sort((a, b) => time(a) - time(b));
		goto(resolve('/compare/[laps]', { laps: ordered.join(',') }));
	}

	async function remove() {
		if (!detail) return;
		if (!confirm(`Delete this session with ${detail.laps.length} laps? This cannot be undone.`))
			return;
		await client.deleteSession(detail.summary.id);
		goto(resolve('/'));
	}

	const exportScopes: ExportScope[] = $derived.by(() => {
		if (!detail) return [];
		const s = detail.summary;
		const base = fileName(s.trackName, s.carName, fileDate(s.startedAt));
		const scopes: ExportScope[] = [];
		if (selected.length > 0) {
			const numbers = detail.laps
				.filter((l) => selected.includes(l.id))
				.map((l) => l.number)
				.sort((a, b) => a - b);
			scopes.push({
				label:
					selected.length === 1
						? `Selected lap (${numbers[0]})`
						: `${selected.length} selected laps`,
				selection: { lapIds: [...selected] },
				csvLapIds: [...selected],
				fileName: selected.length === 1 ? `${base} lap ${numbers[0]}` : base
			});
		}
		scopes.push({
			label: `Whole session (${detail.laps.length} laps)`,
			selection: { sessionIds: [s.id] },
			csvLapIds: detail.laps.map((l) => l.id),
			fileName: base
		});
		return scopes;
	});

	function status(l: LapSummary): string[] {
		const tags: string[] = [];
		if (l.outLap) tags.push('out-lap');
		if (l.inLap) tags.push('in-lap');
		if (l.flashback) tags.push('flashback');
		return tags;
	}
</script>

{#if error}
	<p class="text-red-400" role="alert">Could not load session: {error}</p>
{:else if !detail}
	<p class="text-zinc-400">Loading…</p>
{:else}
	{@const s = detail.summary}
	<a href={resolve('/')} class="text-sm text-zinc-400 hover:text-zinc-200">← Sessions</a>
	<div class="mt-2 mb-4 flex flex-wrap items-start justify-between gap-4">
		<div>
			<h1 class="text-2xl font-semibold">{s.trackName}</h1>
			<p class="text-zinc-300">
				{s.carName}
				{#if formatOrigin(s.origin)}
					<span class="ml-1 rounded bg-sky-950 px-1.5 text-xs text-sky-300"
						>{formatOrigin(s.origin)}</span
					>
				{/if}
			</p>
			<p class="mt-1 text-sm text-zinc-400">
				{formatGame(s.game)} · {formatSessionType(s.sessionType)} · {formatDateTime(s.startedAt)}
				{#if detail.conditions.airTempC !== null}
					· air {detail.conditions.airTempC.toFixed(1)} °C
				{/if}
				{#if detail.conditions.trackTempC !== null}
					· track {detail.conditions.trackTempC.toFixed(1)} °C
				{/if}
				{#if detail.conditions.weather}
					· {detail.conditions.weather}
				{/if}
			</p>
		</div>
		<div class="flex gap-2">
			<button
				type="button"
				class="rounded bg-zinc-100 px-3 py-1.5 text-sm font-medium text-zinc-900 hover:bg-white disabled:opacity-40"
				disabled={selected.length === 0}
				onclick={compare}
			>
				{selected.length > 1 ? `Compare ${selected.length} laps` : 'Show lap'}
			</button>
			{#if client.capabilities.exportFiles}
				<button
					type="button"
					class="rounded border border-zinc-700 px-3 py-1.5 text-sm text-zinc-300 hover:bg-zinc-800"
					onclick={() => (exporting = true)}
				>
					Export…
				</button>
			{/if}
			<button
				type="button"
				class="rounded border border-zinc-700 px-3 py-1.5 text-sm text-zinc-300 hover:border-red-700 hover:text-red-300"
				onclick={remove}
			>
				Delete session
			</button>
		</div>
	</div>

	{#if client.capabilities.exportFiles}
		<ExportDialog bind:open={exporting} scopes={exportScopes} />
	{/if}

	<table class="w-full text-left text-sm">
		<thead class="text-zinc-400">
			<tr>
				<th class="w-8 py-2"><span class="sr-only">Select</span></th>
				<th class="font-normal">Lap</th>
				<th class="text-right font-normal">Time</th>
				<th class="text-right font-normal">Δ best</th>
				<th class="pl-6 font-normal">Status</th>
			</tr>
		</thead>
		<tbody>
			{#each visibleLaps as l (l.id)}
				{@const isBest = best?.id === l.id}
				<tr
					class="border-t border-zinc-800 hover:bg-zinc-900"
					class:opacity-60={!l.valid || l.incomplete}
				>
					<td class="py-1.5">
						<input
							type="checkbox"
							aria-label="Select lap {l.number}"
							checked={selected.includes(l.id)}
							disabled={!selected.includes(l.id) && selected.length >= MAX_COMPARE}
							onchange={() => toggle(l.id)}
						/>
					</td>
					<td>{l.number}</td>
					<td class="text-right font-mono" class:text-purple-300={isBest}>
						{formatLapTime(l.lapTimeMs)}
					</td>
					<td class="text-right font-mono text-zinc-400">
						{#if best && l.lapTimeMs !== null && !isBest}
							{formatDelta(l.lapTimeMs - best.lapTimeMs!)}
						{/if}
					</td>
					<td class="pl-6">
						{#if isBest}<span class="mr-1 rounded bg-purple-900 px-1.5 text-xs text-purple-200"
								>best</span
							>{/if}
						{#each status(l) as tag (tag)}
							<span class="mr-1 rounded bg-zinc-800 px-1.5 text-xs text-zinc-300">{tag}</span>
						{/each}
						{#if !l.valid && l.invalidReason}
							<span class="text-xs text-amber-400/80">{l.invalidReason}</span>
						{/if}
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
	{#if hiddenCount > 0 || showAll}
		<button
			type="button"
			class="mt-3 text-sm text-zinc-400 hover:text-zinc-200"
			onclick={() => (showAll = !showAll)}
		>
			{showAll ? 'Hide incomplete laps' : `Show ${hiddenCount} incomplete lap(s)`}
		</button>
	{/if}
{/if}
