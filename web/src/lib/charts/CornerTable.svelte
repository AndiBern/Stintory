<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!--
	The corners of the comparison: where a lap loses time to the reference and
	what it does differently there. Clicking a corner shows it in the charts
	and on the map.
-->
<script lang="ts">
	import type { CompareData, Corner } from '#lib/api/index.ts';
	import { lapColor } from './colors.ts';
	import { byTimeLost, cornerRows, formatKmh, formatSeconds, type CornerRow } from './corners.ts';
	import { loadFlag, saveFlag } from '#lib/view-prefs.ts';

	let {
		data,
		labels,
		selected = null,
		onselect
	}: {
		data: CompareData;
		/** short label per lap */
		labels: string[];
		/** number of the corner being looked at */
		selected?: number | null;
		onselect: (corner: Corner) => void;
	} = $props();

	/** rows shown before "Show all" */
	const TOP = 5;
	/** time differences below this are a tie, s */
	const TIE_S = 0.005;

	const comparing = $derived(data.laps.length > 1);
	/** the lap compared with the reference */
	let lap = $state(1);
	let order = $state<'lost' | 'lap'>('lost');
	let all = $state(false);
	/** the table is shown; collapsing is remembered across visits */
	let open = $state(loadFlag('compare.corners.open', true));

	function toggle() {
		open = !open;
		saveFlag('compare.corners.open', open);
	}

	// a lap removed from the comparison falls back to the first one
	$effect(() => {
		if (lap >= data.laps.length) lap = 1;
	});
	const k = $derived(comparing ? Math.min(lap, data.laps.length - 1) : 0);
	const rows = $derived.by(() => {
		const r = cornerRows(data, k);
		return comparing && order === 'lost' ? byTimeLost(r) : r;
	});
	const shown = $derived(all || !comparing ? rows : rows.slice(0, TOP));
	const totalLost = $derived(rows.reduce((sum, r) => sum + (r.lap.timeLostS ?? 0), 0));

	function lostClass(s: number | null): string {
		if (s == null || Math.abs(s) < TIE_S) return 'text-zinc-400';
		return s > 0 ? 'text-red-300' : 'text-emerald-300';
	}

	function name(r: CornerRow): string {
		return `${r.corner.number} ${r.corner.direction === 'left' ? 'left' : 'right'}`;
	}

	const tab =
		'rounded px-2 py-1 text-zinc-400 hover:text-zinc-100 aria-pressed:bg-zinc-700 aria-pressed:text-zinc-100';
</script>

{#if data.corners && rows.length > 0}
	<section class="mb-5" aria-labelledby="corners-title">
		<div class="mb-2 flex flex-wrap items-center gap-x-4 gap-y-2">
			<h2 id="corners-title" class="text-lg font-semibold">
				<button
					type="button"
					class="flex items-center gap-1.5 hover:text-zinc-50"
					aria-expanded={open}
					aria-controls="corners-body"
					onclick={toggle}
				>
					<svg
						viewBox="0 0 24 24"
						class="h-4 w-4 text-zinc-400 transition-transform"
						class:-rotate-90={!open}
						fill="none"
						stroke="currentColor"
						stroke-width="2.5"
						stroke-linecap="round"
						stroke-linejoin="round"
						aria-hidden="true"><path d="m6 9 6 6 6-6" /></svg
					>Corners
				</button>
			</h2>
			{#if comparing}
				{#if open && data.laps.length > 2}
					<div
						class="flex rounded border border-zinc-800 p-0.5 text-xs"
						role="group"
						aria-label="Lap compared with the reference"
					>
						{#each data.laps as l, i (l.lapId)}
							{#if i > 0}
								<button
									type="button"
									class="{tab} flex items-center gap-1.5"
									aria-pressed={k === i}
									onclick={() => (lap = i)}
								>
									<span class="inline-block h-0.5 w-3 rounded" style:background={lapColor(i)}
									></span>{labels[i]}
								</button>
							{/if}
						{/each}
					</div>
				{/if}
				{#if open}
					<div
						class="flex rounded border border-zinc-800 p-0.5 text-xs"
						role="group"
						aria-label="Order"
					>
						<button
							type="button"
							class={tab}
							aria-pressed={order === 'lost'}
							onclick={() => (order = 'lost')}>Most time lost</button
						>
						<button
							type="button"
							class={tab}
							aria-pressed={order === 'lap'}
							onclick={() => (order = 'lap')}>Lap order</button
						>
					</div>
				{/if}
				<span class="text-xs text-zinc-500">
					{labels[k]} vs {labels[0]}:
					<span class="font-mono {lostClass(totalLost)} tabular-nums"
						>{formatSeconds(totalLost)} s</span
					>
				</span>
			{/if}
		</div>

		{#if open}
			<div id="corners-body">
				<div class="overflow-x-auto">
					<table class="text-left text-sm" class:w-full={comparing}>
						<thead class="text-zinc-400">
							<tr>
								<th class="py-2 pr-3 font-normal">Corner</th>
								{#if comparing}
									<th class="pr-3 text-right font-normal">Time lost</th>
									<th class="pr-3 text-right font-normal" title="Up to the apex / after the apex"
										>Entry / exit</th
									>
									<th class="pr-3 text-right font-normal" title="Lowest speed, km/h (reference)"
										>Apex</th
									>
									<th
										class="pr-3 text-right font-normal"
										title="Speed at the corner's end, km/h (reference)">Exit</th
									>
									<th class="font-normal">Difference to the reference</th>
								{:else}
									<th class="pl-6 text-right font-normal">Time</th>
									<th class="pl-6 text-right font-normal" title="Distance from the line">Braking</th
									>
									<th class="pl-6 text-right font-normal" title="Lowest speed, km/h">Apex</th>
									<th class="pl-6 text-right font-normal">Gear</th>
									<th class="pl-6 text-right font-normal" title="Speed at the corner's end, km/h"
										>Exit</th
									>
								{/if}
							</tr>
						</thead>
						<tbody>
							{#each shown as r (r.corner.number)}
								<tr
									class="border-t border-zinc-800 hover:bg-zinc-900"
									class:bg-zinc-900={selected === r.corner.number}
								>
									<td class="py-1.5 pr-3 whitespace-nowrap">
										<button
											type="button"
											class="text-left text-zinc-200 underline decoration-zinc-600 underline-offset-2 hover:text-zinc-50 hover:decoration-zinc-300"
											title="Show in the charts and on the map ({Math.round(r.corner.apexM)} m)"
											aria-current={selected === r.corner.number ? 'true' : undefined}
											onclick={() => onselect(r.corner)}
										>
											{name(r)}{#if r.lap.flatOut}<span class="ml-1.5 text-xs text-zinc-500"
													>flat</span
												>{/if}
										</button>
									</td>
									{#if comparing}
										<td class="pr-3 text-right font-mono tabular-nums {lostClass(r.lap.timeLostS)}">
											{formatSeconds(r.lap.timeLostS)}
										</td>
										<td
											class="pr-3 text-right font-mono text-xs whitespace-nowrap text-zinc-400 tabular-nums"
										>
											{formatSeconds(r.lap.entryLostS)} / {formatSeconds(r.lap.exitLostS)}
										</td>
										<td class="pr-3 text-right font-mono whitespace-nowrap tabular-nums">
											{formatKmh(r.lap.minSpeedMps)}
											<span class="text-xs text-zinc-500">({formatKmh(r.ref.minSpeedMps)})</span>
										</td>
										<td class="pr-3 text-right font-mono whitespace-nowrap tabular-nums">
											{formatKmh(r.lap.exitSpeedMps)}
											<span class="text-xs text-zinc-500">({formatKmh(r.ref.exitSpeedMps)})</span>
										</td>
										<td class="text-zinc-300">
											{r.notes.length > 0 ? r.notes.join(' · ') : '–'}
										</td>
									{:else}
										<td class="pl-6 text-right font-mono tabular-nums">
											{r.lap.timeS == null ? '–' : `${r.lap.timeS.toFixed(3)} s`}
										</td>
										<td class="pl-6 text-right font-mono tabular-nums">
											{r.lap.brakeM == null ? '–' : `${Math.round(r.lap.brakeM)} m`}
										</td>
										<td class="pl-6 text-right font-mono tabular-nums"
											>{formatKmh(r.lap.minSpeedMps)}</td
										>
										<td class="pl-6 text-right font-mono tabular-nums"
											>{r.lap.minSpeedGear ?? '–'}</td
										>
										<td class="pl-6 text-right font-mono tabular-nums"
											>{formatKmh(r.lap.exitSpeedMps)}</td
										>
									{/if}
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
				{#if comparing && rows.length > TOP}
					<button
						type="button"
						class="mt-1 text-xs text-zinc-400 hover:text-zinc-200"
						onclick={() => (all = !all)}
					>
						{all ? 'Show fewer' : `Show all ${rows.length} corners`}
					</button>
				{/if}
				<p class="mt-1 text-xs text-zinc-500">
					Each corner counts from before braking to the next corner, the straight after it included,
					so the times add up to the lap. Corners are numbered by Stintory, not the track’s official
					turns.
				</p>
			</div>
		{/if}
	</section>
{/if}
