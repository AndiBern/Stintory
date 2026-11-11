<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { api, type CompareData, type LapTrace } from '#lib/api/index.ts';
	import LapCharts from '#lib/charts/LapCharts.svelte';
	import TrackMap from '#lib/charts/TrackMap.svelte';
	import { lapColor } from '#lib/charts/colors.ts';
	import ReferencePicker, { type ReferenceTarget } from '#lib/components/ReferencePicker.svelte';
	import { formatDelta, formatLapTime } from '#lib/format.ts';

	const MAX_COMPARE = 8;
	const client = api();
	const lapIds = $derived((page.params.laps ?? '').split(',').filter(Boolean));

	let data = $state<CompareData | null>(null);
	let error = $state<string | null>(null);
	let cursor = $state<number | null>(null);
	let picking = $state<ReferenceTarget | null>(null);
	let pickError = $state<string | null>(null);

	$effect(() => {
		const ids = lapIds;
		error = null;
		if (ids.length === 0) {
			data = null;
			error = 'No laps selected.';
			return;
		}
		client
			.compareLaps({ lapIds: ids })
			.then((d) => (data = d))
			.catch((e) => {
				data = null;
				error = e.message;
			});
	});

	function lapLabel(l: LapTrace): string {
		const lap = `Lap ${l.number} (${formatLapTime(l.lapTimeMs)})`;
		return l.driver ? `${l.driver} · ${lap}` : lap;
	}

	const labels = $derived(data ? data.laps.map(lapLabel) : []);
	/** Set for laps someone shared: keeps "Sharing" highlighted in the menu */
	const ownerParam = $derived(page.url.searchParams.get('owner'));
	/** Back to the session of the first own lap, else of the reference. */
	const home = $derived(data ? (data.laps.find((l) => !l.driver) ?? data.laps[0]) : undefined);
	const sessionHref = $derived.by(() => {
		if (!home) return null;
		const href = resolve('/session/[id]', { id: home.sessionId });
		// server: sessions others shared are opened with their owner
		const owner = home.owner?.id;
		return owner ? `${href}?owner=${encodeURIComponent(owner)}` : href;
	});

	/** Values at the cursor for the legend. */
	function at(channel: string, lapIndex: number): number | null {
		if (!data || cursor === null) return null;
		return data.laps[lapIndex].channels[channel]?.[cursor] ?? null;
	}

	function show(ids: string[]) {
		const query = ownerParam ? `?owner=${encodeURIComponent(ownerParam)}` : '';
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- resolved path + query
		goto(resolve('/compare/[laps]', { laps: ids.join(',') }) + query);
	}

	function makeReference(id: string) {
		show([id, ...lapIds.filter((l) => l !== id)]);
	}

	function remove(id: string) {
		show(lapIds.filter((l) => l !== id));
	}

	/** Opens the picker for the reference lap's track and car. */
	async function addLaps() {
		const first = data?.laps[0];
		if (!first) return;
		pickError = null;
		try {
			const s = (await client.getSession(first.sessionId, first.owner?.id)).summary;
			picking = {
				trackKey: s.trackKey,
				carKey: s.carKey,
				trackName: s.trackName,
				carName: s.carName
			};
		} catch (e) {
			pickError = e instanceof Error ? e.message : String(e);
		}
	}
</script>

{#if sessionHref}
	<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- resolved path + query -->
	<a href={sessionHref} class="text-sm text-zinc-400 hover:text-zinc-200">← Session</a>
{/if}
<div class="mt-2 mb-4 flex flex-wrap items-center justify-between gap-2">
	<h1 class="text-2xl font-semibold">
		{lapIds.length > 1 ? 'Lap comparison' : 'Lap'}
	</h1>
	{#if data}
		<button
			type="button"
			class="rounded border border-zinc-700 px-3 py-1.5 text-sm text-zinc-300 hover:bg-zinc-800 disabled:opacity-40"
			disabled={lapIds.length >= MAX_COMPARE}
			onclick={addLaps}
		>
			Add laps…
		</button>
	{/if}
</div>

{#if pickError}
	<p class="mb-2 text-sm text-red-400" role="alert">{pickError}</p>
{/if}

{#if error}
	<p class="text-red-400" role="alert">Could not load laps: {error}</p>
{:else if !data}
	<p class="text-zinc-400">Loading…</p>
{:else}
	<ul class="mb-4 flex flex-wrap gap-x-6 gap-y-2 text-sm" aria-label="Laps">
		{#each data.laps as lap, i (lap.lapId)}
			<li class="flex items-center gap-2">
				<span class="inline-block h-1 w-5 rounded" style:background={lapColor(i)}></span>
				<span>{labels[i]}</span>
				{#if i === 0 && data.laps.length > 1}
					<span class="rounded bg-zinc-800 px-1.5 text-xs text-zinc-300">reference</span>
				{/if}
				{#if !lap.valid}<span class="text-xs text-amber-400/80">invalid</span>{/if}
				{#if cursor !== null}
					<span class="font-mono text-xs text-zinc-400">
						{(at('speed', i) ?? NaN) * 3.6 > 0 ? `${Math.round(at('speed', i)! * 3.6)} km/h` : ''}
						{#if i > 0 && lap.deltaS[cursor] !== null}
							{formatDelta(lap.deltaS[cursor]! * 1000)}
						{/if}
					</span>
				{/if}
				{#if i > 0}
					<button
						type="button"
						class="text-xs text-zinc-500 hover:text-zinc-200"
						aria-label="Use {labels[i]} as reference"
						onclick={() => makeReference(lap.lapId)}>Use as reference</button
					>
				{/if}
				{#if data.laps.length > 1}
					<button
						type="button"
						class="text-xs text-zinc-500 hover:text-red-300"
						aria-label="Remove {labels[i]}"
						onclick={() => remove(lap.lapId)}>✕</button
					>
				{/if}
			</li>
		{/each}
	</ul>

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

<!-- added laps go after the current ones; the reference stays -->
<ReferencePicker
	bind:target={picking}
	exclude={lapIds}
	max={MAX_COMPARE - lapIds.length}
	onpick={(laps) => show([...lapIds, ...laps.map((l) => l.lapId)])}
/>
