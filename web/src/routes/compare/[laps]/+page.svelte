<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { untrack } from 'svelte';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { api, type CompareData, type LapTrace } from '#lib/api/index.ts';
	import CompareCharts from '#lib/charts/CompareCharts.svelte';
	import FollowMap from '#lib/charts/FollowMap.svelte';
	import { lapColor } from '#lib/charts/colors.ts';
	import type { XRange } from '#lib/charts/compare-panels.ts';
	import {
		firstIndex,
		indexAtTime,
		lastIndex,
		rebaseDeltas,
		referenceTime,
		sectionOf,
		sectionTime,
		type OffsetMode
	} from '#lib/charts/compare-view.ts';
	import ReferencePicker, { type ReferenceTarget } from '#lib/components/ReferencePicker.svelte';
	import { formatLapTime } from '#lib/format.ts';

	const MAX_COMPARE = 8;
	const client = api();
	const lapIds = $derived((page.params.laps ?? '').split(',').filter(Boolean));

	let data = $state<CompareData | null>(null);
	let error = $state<string | null>(null);
	/** grid index of the reference car: map, slider and chart markers */
	let cursor = $state(0);
	let mode = $state<OffsetMode>('distance');
	let playing = $state(false);
	let speed = $state(1);
	/** the part of the lap looked at (m); null = the full lap */
	let range = $state<XRange>(null);
	const SPEEDS = [0.25, 0.5, 1, 2];
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
			.then((d) => {
				data = d;
				range = null;
				cursor = firstIndex(d);
				playing = false;
			})
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
	/** for the map and the chart tooltips */
	const shortLabels = $derived(
		data ? data.laps.map((l) => (l.driver ? `${l.driver} · L${l.number}` : `Lap ${l.number}`)) : []
	);
	const first = $derived(data ? firstIndex(data) : 0);
	const last = $derived(data ? lastIndex(data) : 0);
	const refTime = $derived(data ? referenceTime(data, cursor) : null);
	/** grid indices of the section; the slider and playback stay inside */
	const section = $derived(data ? sectionOf(data, range) : { start: 0, end: 0 });
	const sectionStart = $derived(section.start);
	const inSection = $derived(range !== null);
	/** in a section, the diff to the reference counts from its start */
	const view = $derived(data && inSection ? rebaseDeltas(data, sectionStart) : data);
	/** each lap's time through the section */
	const sectionTimes = $derived(
		data && range ? data.laps.map((l) => sectionTime(l, section)) : null
	);

	// the cursor stays in the section
	$effect(() => {
		const { start, end } = section;
		if (cursor < start || cursor > end) cursor = start;
	});

	const at = (i: number) => data?.distM[i] ?? 0;

	/** Starts the section at the cursor (keeps its end if that's after). */
	function startHere() {
		const end = cursor < section.end ? section.end : last;
		if (cursor >= end) return;
		range = { min: at(cursor), max: at(end) };
	}

	/** Ends the section at the cursor (keeps its start if that's before). */
	function endHere() {
		const start = cursor > section.start ? section.start : first;
		if (cursor <= start) return;
		range = { min: at(start), max: at(cursor) };
	}

	/** Moving through the lap by hand stops the playback. */
	function seek(index: number) {
		playing = false;
		cursor = index;
	}

	/** Plays the lap in the reference lap's time. */
	$effect(() => {
		if (!playing || !data) return;
		const d = data;
		const rate = speed;
		const { start, end } = untrack(() => section);
		let t = untrack(() => referenceTime(d, cursor >= end ? start : cursor)) ?? 0;
		let previous = performance.now();
		let frame = 0;
		const tick = (now: number) => {
			t += ((now - previous) / 1000) * rate;
			previous = now;
			const i = indexAtTime(d.laps[0].channels.t, t);
			if (i === null || i >= end) {
				cursor = end;
				playing = false;
				return;
			}
			cursor = Math.round(i);
			frame = requestAnimationFrame(tick);
		};
		frame = requestAnimationFrame(tick);
		return () => cancelAnimationFrame(frame);
	});
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
				{#if sectionTimes}
					{@const own = sectionTimes[i]}
					{@const ref = sectionTimes[0]}
					<span
						class="font-mono text-xs text-zinc-300 tabular-nums"
						title="Time through the section"
					>
						{own === null ? '–' : `${own.toFixed(3)} s`}{#if i > 0 && own !== null && ref !== null}
							<span class="ml-1 text-zinc-500">
								({own - ref >= 0 ? '+' : '−'}{Math.abs(own - ref).toFixed(3)})</span
							>{/if}
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

	<FollowMap
		data={view ?? data}
		{cursor}
		labels={shortLabels}
		section={range ? section : null}
		bind:mode
	/>

	<!-- move through the lap: by hand or played back in the reference lap's time -->
	<div class="my-4 flex flex-wrap items-center gap-3">
		<button
			type="button"
			class="flex h-8 w-8 items-center justify-center rounded border border-zinc-700 text-zinc-200 hover:bg-zinc-800"
			aria-label={playing ? 'Pause' : 'Play'}
			onclick={() => (playing = !playing)}
		>
			{#if playing}
				<svg viewBox="0 0 24 24" class="h-4 w-4" fill="currentColor" aria-hidden="true"
					><rect x="6" y="5" width="4" height="14" rx="1" /><rect
						x="14"
						y="5"
						width="4"
						height="14"
						rx="1"
					/></svg
				>
			{:else}
				<svg viewBox="0 0 24 24" class="h-4 w-4" fill="currentColor" aria-hidden="true"
					><path
						d="M7 5.5v13a1 1 0 0 0 1.5.86l11-6.5a1 1 0 0 0 0-1.72l-11-6.5A1 1 0 0 0 7 5.5z"
					/></svg
				>
			{/if}
		</button>
		<div class="flex rounded border border-zinc-800 p-0.5 text-xs" role="group" aria-label="Speed">
			{#each SPEEDS as s (s)}
				<button
					type="button"
					class="rounded px-2 py-1 text-zinc-400 hover:text-zinc-100 aria-pressed:bg-zinc-700 aria-pressed:text-zinc-100"
					aria-pressed={speed === s}
					onclick={() => (speed = s)}>{s}×</button
				>
			{/each}
		</div>
		<input
			type="range"
			class="min-w-48 flex-1 accent-zinc-200"
			min={section.start}
			max={section.end}
			step="1"
			value={cursor}
			aria-label="Position in the lap"
			aria-valuetext="{Math.round(data.distM[cursor] ?? 0)} m"
			oninput={(e) => seek(Number(e.currentTarget.value))}
		/>
		<span class="w-36 text-right font-mono text-xs text-zinc-400 tabular-nums">
			{Math.round(data.distM[cursor] ?? 0)} m · {refTime === null
				? '–'
				: formatLapTime(Math.round(refTime * 1000))}
		</span>
	</div>

	<!-- a part of the lap: start and end at the cursor, or drag across a chart -->
	<div class="mb-5 flex flex-wrap items-center gap-2 text-xs" role="group" aria-label="Section">
		<span class="text-zinc-500">Section</span>
		<button
			type="button"
			class="rounded border border-zinc-700 px-2 py-1 text-zinc-300 hover:bg-zinc-800 disabled:opacity-40"
			disabled={cursor >= last}
			onclick={startHere}>Start here</button
		>
		<button
			type="button"
			class="rounded border border-zinc-700 px-2 py-1 text-zinc-300 hover:bg-zinc-800 disabled:opacity-40"
			disabled={cursor <= first}
			onclick={endHere}>End here</button
		>
		{#if range}
			<span class="font-mono text-zinc-200 tabular-nums">
				{Math.round(at(section.start))}–{Math.round(at(section.end))} m
			</span>
			<span class="text-zinc-500">({Math.round(at(section.end) - at(section.start))} m)</span>
			<button
				type="button"
				class="rounded border border-zinc-700 px-2 py-1 text-zinc-300 hover:bg-zinc-800"
				onclick={() => (range = null)}>Full lap</button
			>
			<span class="text-zinc-500">Diff to ref counts from the section start.</span>
		{:else}
			<span class="text-zinc-500">Full lap · or drag across a chart</span>
		{/if}
	</div>

	<CompareCharts data={view ?? data} {cursor} labels={shortLabels} bind:range onCursor={seek} />
{/if}

<!-- added laps go after the current ones; the reference stays -->
<ReferencePicker
	bind:target={picking}
	exclude={lapIds}
	max={MAX_COMPARE - lapIds.length}
	onpick={(laps) => show([...lapIds, ...laps.map((l) => l.lapId)])}
/>
