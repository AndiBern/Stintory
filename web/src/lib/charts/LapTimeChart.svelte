<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!--
	Lap time per lap of a session. Clean laps are filled dots, invalid, out- and
	in-laps hollow ones, the best lap purple (as in the lap table). Laps far
	slower than the rest sit as a triangle at the top edge, far faster ones
	(partial laps) at the bottom, so one out-lap doesn't squash the scale; the
	tooltip has their time.
-->
<script lang="ts">
	import type { LapSummary } from '#lib/api/index.ts';
	import { formatDelta, formatLapTime } from '#lib/format.ts';
	import { formatTick, isClean, lapTicks, timeScale } from './lap-times.ts';

	let { laps, bestId }: { laps: LapSummary[]; bestId?: string } = $props();

	const HEIGHT = 200;
	const M = { top: 14, right: 16, bottom: 26, left: 58 };

	let width = $state(0);
	let hover = $state<number | null>(null);

	const timed = $derived(laps.filter((l) => l.lapTimeMs !== null));
	const best = $derived(timed.find((l) => l.id === bestId));
	const scale = $derived(timeScale(timed, 6));
	const first = $derived(timed[0]?.number ?? 1);
	const last = $derived(timed.at(-1)?.number ?? 1);
	const plotW = $derived(Math.max(width - M.left - M.right, 1));
	const plotH = HEIGHT - M.top - M.bottom;

	function x(n: number): number {
		return last === first ? M.left + plotW / 2 : M.left + ((n - first) / (last - first)) * plotW;
	}
	function y(ms: number): number {
		if (!scale) return M.top;
		return M.top + (1 - (ms - scale.min) / (scale.max - scale.min)) * plotH;
	}
	const above = (l: LapSummary) => scale !== null && l.lapTimeMs! > scale.max;
	const below = (l: LapSummary) => scale !== null && l.lapTimeMs! < scale.min;
	const offScale = (l: LapSummary) => above(l) || below(l);

	/** Line segments between neighbouring laps that are both on the scale. */
	const path = $derived.by(() => {
		let d = '';
		let prev: LapSummary | null = null;
		for (const l of timed) {
			const joined = prev && prev.number === l.number - 1 && !offScale(prev) && !offScale(l);
			if (!offScale(l)) d += `${joined ? 'L' : 'M'}${x(l.number)},${y(l.lapTimeMs!)}`;
			prev = l;
		}
		return d;
	});

	const xTicks = $derived(lapTicks(first, last, Math.max(2, Math.floor(plotW / 36))));
	const hasOffScale = $derived(timed.some(offScale));
	const hasUnclean = $derived(timed.some((l) => !isClean(l)));
	const hovered = $derived(hover === null ? null : timed[hover]);

	/** The crosshair snaps to the lap nearest to the pointer. */
	function onMove(e: PointerEvent) {
		const rect = (e.currentTarget as SVGElement).getBoundingClientRect();
		const px = e.clientX - rect.left;
		let nearest = 0;
		timed.forEach((l, i) => {
			if (Math.abs(x(l.number) - px) < Math.abs(x(timed[nearest].number) - px)) nearest = i;
		});
		hover = nearest;
	}

	function lapLine(l: LapSummary): string {
		const delta =
			best && l.id !== best.id && !l.outLap
				? ` · ${formatDelta(l.lapTimeMs! - best.lapTimeMs!)}`
				: '';
		return `Lap ${l.number}${delta}`;
	}

	function tags(l: LapSummary): string {
		const t: string[] = [];
		if (l.id === bestId) t.push('best');
		if (l.outLap) t.push('out-lap');
		if (l.inLap) t.push('in-lap');
		if (!l.valid) t.push(l.invalidReason ? `invalid: ${l.invalidReason}` : 'invalid');
		return t.join(' · ');
	}

	const summary = $derived(
		`Lap times of ${timed.length} laps` +
			(best ? `, best ${formatLapTime(best.lapTimeMs)} in lap ${best.number}` : '') +
			'. All values are in the lap table below.'
	);
</script>

<figure class="mb-6">
	<figcaption class="mb-2 flex flex-wrap items-baseline justify-between gap-x-6 gap-y-1">
		<span class="text-sm font-medium text-zinc-200">Session performance</span>
		<span class="flex flex-wrap gap-x-4 text-xs text-zinc-400" aria-hidden="true">
			<span class="inline-flex items-center gap-1.5">
				<svg width="10" height="10"><circle cx="5" cy="5" r="4" class="fill-zinc-100" /></svg>Clean
				lap
			</span>
			<span class="inline-flex items-center gap-1.5">
				<svg width="10" height="10"><circle cx="5" cy="5" r="4" class="fill-purple-400" /></svg>Best
			</span>
			{#if hasUnclean}
				<span class="inline-flex items-center gap-1.5">
					<svg width="10" height="10">
						<circle
							cx="5"
							cy="5"
							r="3.5"
							class="fill-zinc-950 stroke-zinc-400"
							stroke-width="1.5"
						/>
					</svg>Invalid, out- or in-lap
				</span>
			{/if}
			{#if hasOffScale}
				<span class="inline-flex items-center gap-1.5">
					<svg width="10" height="10">
						<path d="M5 1 L9 8 L1 8 Z" class="fill-zinc-950 stroke-zinc-400" stroke-width="1.5" />
					</svg>Off the scale (much slower or faster)
				</span>
			{/if}
		</span>
	</figcaption>

	<div class="relative" bind:clientWidth={width}>
		{#if width > 0 && scale}
			<svg
				{width}
				height={HEIGHT}
				role="img"
				aria-label={summary}
				class="block touch-none select-none"
				onpointermove={onMove}
				onpointerleave={() => (hover = null)}
			>
				<!-- grid and time axis -->
				{#each scale.ticks as t (t)}
					<line
						x1={M.left}
						x2={M.left + plotW}
						y1={y(t)}
						y2={y(t)}
						class="stroke-zinc-800"
						shape-rendering="crispEdges"
					/>
					<text
						x={M.left - 8}
						y={y(t)}
						text-anchor="end"
						dominant-baseline="middle"
						class="fill-zinc-400 text-[11px] tabular-nums"
					>
						{formatTick(t, scale.step)}
					</text>
				{/each}
				<!-- lap axis -->
				{#each xTicks as n (n)}
					<text
						x={x(n)}
						y={HEIGHT - 8}
						text-anchor="middle"
						class="fill-zinc-400 text-[11px] tabular-nums"
					>
						{n}
					</text>
				{/each}

				{#if hovered}
					<line
						x1={x(hovered.number)}
						x2={x(hovered.number)}
						y1={M.top}
						y2={M.top + plotH}
						class="stroke-zinc-600"
						shape-rendering="crispEdges"
					/>
				{/if}

				<path
					d={path}
					fill="none"
					class="stroke-zinc-500"
					stroke-width="2"
					stroke-linejoin="round"
					stroke-linecap="round"
				/>

				{#each timed as l, i (l.id)}
					{@const cx = x(l.number)}
					{@const r = hover === i ? 5.5 : 4}
					{#if offScale(l)}
						{@const edge = above(l) ? M.top : M.top + plotH}
						{@const tip = above(l) ? -4 : 4}
						<path
							d="M{cx} {edge + tip} L{cx + 5} {edge - tip * 1.25} L{cx - 5} {edge - tip * 1.25} Z"
							class="fill-zinc-950 stroke-zinc-400"
							stroke-width="1.5"
							stroke-linejoin="round"
						/>
					{:else if l.id === bestId}
						<circle
							{cx}
							cy={y(l.lapTimeMs!)}
							r={r + 1}
							class="fill-purple-400 stroke-zinc-950"
							stroke-width="2"
						/>
					{:else if isClean(l)}
						<circle
							{cx}
							cy={y(l.lapTimeMs!)}
							{r}
							class="fill-zinc-100 stroke-zinc-950"
							stroke-width="2"
						/>
					{:else}
						<circle
							{cx}
							cy={y(l.lapTimeMs!)}
							r={r - 0.5}
							class="fill-zinc-950 stroke-zinc-400"
							stroke-width="1.5"
						/>
					{/if}
				{/each}

				<!-- the one direct label: the best lap -->
				{#if best && !offScale(best) && hovered?.id !== best.id}
					{@const bx = x(best.number)}
					{@const right = bx < M.left + plotW - 90}
					<text
						x={right ? bx + 10 : bx - 10}
						y={y(best.lapTimeMs!) + 14}
						text-anchor={right ? 'start' : 'end'}
						class="fill-zinc-300 font-mono text-[11px]"
					>
						{formatLapTime(best.lapTimeMs)}
					</text>
				{/if}
			</svg>

			{#if hovered}
				{@const left = x(hovered.number)}
				<div
					class="pointer-events-none absolute top-0 z-10 rounded border border-zinc-700 bg-zinc-900 px-2.5 py-1.5 text-xs whitespace-nowrap shadow-lg"
					style:left="{left}px"
					style:transform={left > width / 2 ? 'translateX(calc(-100% - 12px))' : 'translateX(12px)'}
				>
					<div class="font-mono text-sm font-semibold text-zinc-100">
						{formatLapTime(hovered.lapTimeMs)}
					</div>
					<div class="text-zinc-400">{lapLine(hovered)}</div>
					{#if tags(hovered)}<div class="text-zinc-400">{tags(hovered)}</div>{/if}
				</div>
			{/if}
		{/if}
	</div>
</figure>
