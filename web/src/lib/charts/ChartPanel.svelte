<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!--
	One chart of the comparison page over the lap distance. The marker at the
	cursor is always shown, with a tooltip listing every lap's value; hovering
	moves the cursor (for all charts and the map). Drag to zoom, double-click
	to reset: the zoom is shared by all charts.
-->
<script lang="ts">
	import uPlot from 'uplot';
	import 'uplot/dist/uPlot.min.css';
	import { untrack } from 'svelte';
	import type { CompareData } from '#lib/api/index.ts';
	import { lapColor } from './colors.ts';
	import { formatKm, panelSeries, type Panel, type XRange } from './compare-panels.ts';

	let {
		data,
		panel,
		cursor,
		labels,
		xRange = $bindable(null),
		onCursor
	}: {
		data: CompareData;
		panel: Panel;
		cursor: number;
		/** short label per lap */
		labels: string[];
		/** shared zoom; null = the whole lap */
		xRange?: XRange;
		onCursor: (index: number) => void;
	} = $props();

	let host: HTMLDivElement | undefined = $state();
	let width = $state(0);
	let plot: uPlot | null = null;
	/** plotting area (CSS px, relative to the host) and the scales */
	let geo = $state({ left: 0, top: 0, w: 0, h: 0, xMin: 0, xMax: 1, yMin: 0, yMax: 1 });

	const series = $derived(panelSeries(data, panel));
	const full = $derived({ min: data.distM[0] ?? 0, max: data.distM.at(-1) ?? 1 });

	function readGeo(u: uPlot) {
		const dpr = window.devicePixelRatio || 1;
		geo = {
			left: u.bbox.left / dpr,
			top: u.bbox.top / dpr,
			w: u.bbox.width / dpr,
			h: u.bbox.height / dpr,
			xMin: u.scales.x.min ?? 0,
			xMax: u.scales.x.max ?? 1,
			yMin: u.scales.y.min ?? 0,
			yMax: u.scales.y.max ?? 1
		};
	}

	/** set while applying the shared zoom, so it doesn't echo back */
	let applying = false;

	$effect(() => {
		if (!host) return;
		const el = host;
		const s = series;
		const p = panel;
		const w = untrack(() => width) || el.clientWidth;
		const axis = {
			stroke: '#9f9fa9',
			grid: { stroke: '#2e2e33', width: 1 },
			ticks: { stroke: '#424249', width: 1 },
			font: '11px system-ui, sans-serif'
		};
		const u = new uPlot(
			{
				width: w,
				height: p.height,
				legend: { show: false },
				scales: { x: { time: false }, y: p.range ? { range: p.range } : {} },
				series: [
					{},
					...s.map((x) => ({
						stroke: lapColor(x.lap),
						width: 1.5,
						dash: x.line?.dash || (p.delta && x.lap === 0) ? [4, 3] : undefined,
						paths: p.stepped ? uPlot.paths.stepped?.({ align: 1 }) : undefined,
						points: { show: false }
					}))
				],
				axes: [
					{
						...axis,
						values: (_u, ticks) => {
							const step = ticks.length > 1 ? ticks[1] - ticks[0] : 1000;
							return ticks.map((t) => formatKm(t, step));
						}
					},
					{
						...axis,
						size: 56,
						// whole gears only (R, N, 1, 2, …)
						...(p.key === 'gear' ? { incrs: [1, 2], space: 12 } : {}),
						values: (_u, ticks) => ticks.map((t) => (p.key === 'gear' ? p.format(t) : String(t)))
					}
				],
				cursor: {
					x: false,
					y: false,
					points: { show: false },
					drag: { x: true, y: false }
				},
				hooks: {
					ready: [readGeo],
					setSize: [readGeo],
					setCursor: [
						(u) => {
							const i = u.cursor.idx;
							if (i != null) onCursor(i);
						}
					],
					setScale: [
						(u, key) => {
							readGeo(u);
							if (key !== 'x' || applying) return;
							const min = u.scales.x.min ?? full.min;
							const max = u.scales.x.max ?? full.max;
							const whole = min <= full.min + 1e-6 && max >= full.max - 1e-6;
							xRange = whole ? null : { min, max };
						}
					]
				}
			},
			[data.distM, ...s.map((x) => x.values)] as uPlot.AlignedData,
			el
		);
		plot = u;
		const range = untrack(() => xRange);
		if (range) {
			applying = true;
			u.setScale('x', range);
			applying = false;
		}
		return () => {
			u.destroy();
			plot = null;
		};
	});

	$effect(() => {
		if (plot && width > 0) plot.setSize({ width, height: panel.height });
	});

	// the shared zoom
	$effect(() => {
		const range = xRange ?? full;
		const u = plot;
		if (!u) return;
		if (u.scales.x.min === range.min && u.scales.x.max === range.max) return;
		applying = true;
		u.setScale('x', range);
		applying = false;
	});

	const markerX = $derived(
		geo.left + ((data.distM[cursor] - geo.xMin) / (geo.xMax - geo.xMin || 1)) * geo.w
	);
	const markerShown = $derived(geo.w > 0 && markerX >= geo.left && markerX <= geo.left + geo.w);
	function yPos(v: number): number {
		return geo.top + (1 - (v - geo.yMin) / (geo.yMax - geo.yMin || 1)) * geo.h;
	}
	const rows = $derived(
		series.map((x) => ({
			lap: x.lap,
			label:
				x.line?.name && panel.lines.length > 1 ? `${labels[x.lap]} ${x.line.name}` : labels[x.lap],
			value: x.values[cursor] ?? null
		}))
	);
	const flip = $derived(markerX > geo.left + geo.w * 0.6);
	/** short charts get a wider, flatter tooltip */
	const tooltipRows = $derived(Math.min(rows.length, panel.height < 140 ? 2 : 4));
</script>

<section class="mb-3" aria-label={panel.title}>
	<h3 class="mb-0.5 pl-14 text-xs font-medium text-zinc-300">
		{panel.title}{#if panel.unit}<span class="ml-1 font-normal text-zinc-500">({panel.unit})</span
			>{/if}
	</h3>
	<div class="relative" bind:this={host} bind:clientWidth={width}>
		{#if markerShown}
			<!-- marker: a hairline at the cursor and a dot per lap -->
			<div
				class="pointer-events-none absolute z-10 w-px bg-zinc-400"
				style:left="{markerX}px"
				style:top="{geo.top}px"
				style:height="{geo.h}px"
			></div>
			{#each rows as r, i (i)}
				{#if r.value !== null && r.value >= geo.yMin && r.value <= geo.yMax}
					<div
						class="pointer-events-none absolute z-10 h-2 w-2 -translate-x-1/2 -translate-y-1/2 rounded-full ring-2 ring-zinc-950"
						style:left="{markerX}px"
						style:top="{yPos(r.value)}px"
						style:background={lapColor(r.lap)}
					></div>
				{/if}
			{/each}
			<div
				class="pointer-events-none absolute z-20 grid grid-flow-col gap-x-4 rounded border border-zinc-700 bg-zinc-900/90 px-2 py-1 text-xs whitespace-nowrap shadow-lg"
				style:left="{markerX}px"
				style:top="{geo.top + 2}px"
				style:grid-template-rows="repeat({tooltipRows}, auto)"
				style:transform={flip ? 'translateX(calc(-100% - 10px))' : 'translateX(10px)'}
			>
				{#each rows as r, i (i)}
					<span class="flex items-center gap-1.5 leading-4">
						<span class="inline-block h-0.5 w-3 rounded" style:background={lapColor(r.lap)}></span>
						<span class="font-mono font-semibold text-zinc-100 tabular-nums"
							>{r.value === null ? '–' : panel.format(r.value)}</span
						>
						<span class="text-zinc-500">{r.label}</span>
					</span>
				{/each}
			</div>
		{/if}
	</div>
</section>
