<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { api, type CompareData } from '#lib/api/index.ts';
	import LapCharts from '#lib/charts/LapCharts.svelte';
	import TrackMap from '#lib/charts/TrackMap.svelte';
	import { lapColor } from '#lib/charts/colors.ts';
	import { formatDelta, formatLapTime } from '#lib/format.ts';

	const client = api();
	const lapIds = $derived((page.params.laps ?? '').split(',').filter(Boolean));

	let data = $state<CompareData | null>(null);
	let error = $state<string | null>(null);
	let cursor = $state<number | null>(null);

	$effect(() => {
		const ids = lapIds;
		data = null;
		error = null;
		if (ids.length === 0) {
			error = 'No laps selected.';
			return;
		}
		client
			.compareLaps({ lapIds: ids })
			.then((d) => (data = d))
			.catch((e) => (error = e.message));
	});

	const labels = $derived(
		data ? data.laps.map((l) => `Lap ${l.number} (${formatLapTime(l.lapTimeMs)})`) : []
	);
	const sessionId = $derived(data?.laps[0]?.sessionId);

	/** Values at the cursor for the legend. */
	function at(channel: string, lapIndex: number): number | null {
		if (!data || cursor === null) return null;
		return data.laps[lapIndex].channels[channel]?.[cursor] ?? null;
	}
</script>

{#if sessionId}
	<a
		href={resolve('/session/[id]', { id: sessionId })}
		class="text-sm text-zinc-400 hover:text-zinc-200">← Session</a
	>
{/if}
<h1 class="mt-2 mb-4 text-2xl font-semibold">
	{lapIds.length > 1 ? 'Lap comparison' : 'Lap'}
</h1>

{#if error}
	<p class="text-red-400" role="alert">Could not load laps: {error}</p>
{:else if !data}
	<p class="text-zinc-400">Loading…</p>
{:else}
	<div class="mb-4 flex flex-wrap gap-x-6 gap-y-2 text-sm">
		{#each data.laps as lap, i (lap.lapId)}
			<div class="flex items-center gap-2">
				<span class="inline-block h-1 w-5 rounded" style:background={lapColor(i)}></span>
				<span>{labels[i]}{i === 0 && data.laps.length > 1 ? ' – reference' : ''}</span>
				{#if !lap.valid}<span class="text-xs text-amber-400/80">invalid</span>{/if}
				{#if cursor !== null}
					<span class="font-mono text-xs text-zinc-400">
						{(at('speed', i) ?? NaN) * 3.6 > 0 ? `${Math.round(at('speed', i)! * 3.6)} km/h` : ''}
						{#if i > 0 && lap.deltaS[cursor] !== null}
							{formatDelta(lap.deltaS[cursor]! * 1000)}
						{/if}
					</span>
				{/if}
			</div>
		{/each}
	</div>

	<div class="grid grid-cols-1 gap-6 xl:grid-cols-[1fr_340px]">
		<div class="min-w-0">
			<LapCharts {data} {labels} onCursor={(i) => (cursor = i)} />
			<p class="mt-1 text-xs text-zinc-500">
				Drag to zoom, double-click to reset. Distance from the start/finish line.
			</p>
		</div>
		<div class="xl:sticky xl:top-0 xl:self-start">
			<TrackMap {data} cursorIndex={cursor} />
		</div>
	</div>
{/if}
