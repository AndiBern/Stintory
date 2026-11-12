<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- The comparison page's charts: the main ones, further channels folded away. -->
<script lang="ts">
	import type { CompareData } from '#lib/api/index.ts';
	import ChartPanel from './ChartPanel.svelte';
	import { mainPanels, morePanels, type XRange } from './compare-panels.ts';

	let {
		data,
		cursor,
		labels,
		onCursor
	}: {
		data: CompareData;
		cursor: number;
		labels: string[];
		onCursor: (index: number) => void;
	} = $props();

	let xRange = $state<XRange>(null);
	const main = $derived(mainPanels(data));
	const more = $derived(morePanels(data));
</script>

<div>
	{#each main as panel (panel.key)}
		<ChartPanel {data} {panel} {cursor} {labels} bind:xRange {onCursor} />
	{/each}
	<p class="pl-14 text-xs text-zinc-500">
		Hover to move through the lap · drag to zoom · double-click to reset
	</p>
	{#if more.length > 0}
		<details class="mt-4">
			<summary class="cursor-pointer text-sm text-zinc-400 hover:text-zinc-200"
				>More channels ({more.map((p) => p.title).join(', ')})</summary
			>
			<div class="mt-3">
				{#each more as panel (panel.key)}
					<ChartPanel {data} {panel} {cursor} {labels} bind:xRange {onCursor} />
				{/each}
			</div>
		</details>
	{/if}
</div>
