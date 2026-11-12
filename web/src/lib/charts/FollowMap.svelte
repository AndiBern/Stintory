<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!--
	Track map that follows the reference car: it stays in the centre, the
	other laps' cars move around it (placed by distance or by time, see
	compare-view.ts). Each lap's line in its colour, an overview of the whole
	track in the corner.
-->
<script lang="ts">
	import type { CompareData } from '#lib/api/index.ts';
	import { lapColor } from './colors.ts';
	import {
		carIndex,
		firstIndex,
		gapText,
		headingAt,
		positionAt,
		type OffsetMode,
		type Point
	} from './compare-view.ts';

	let {
		data,
		cursor,
		labels,
		mode = $bindable('distance')
	}: {
		data: CompareData;
		/** grid index of the reference car */
		cursor: number;
		/** short label per lap */
		labels: string[];
		mode?: OffsetMode;
	} = $props();

	const HEIGHT = 440;
	const ZOOM_MIN = 0.3;
	const ZOOM_MAX = 30;

	let canvas: HTMLCanvasElement | undefined = $state();
	let width = $state(0);
	/** pixels per metre */
	let zoom = $state(3);
	let headingUp = $state(false);

	const hasMap = $derived(data.laps.some((l) => l.channels.pos_x && l.channels.pos_y));

	/** Bounds of the reference line for the overview. */
	const bounds = $derived.by(() => {
		let minX = Infinity,
			maxX = -Infinity,
			minY = Infinity,
			maxY = -Infinity;
		const xs = data.laps[0].channels.pos_x ?? [];
		const ys = data.laps[0].channels.pos_y ?? [];
		for (let i = 0; i < xs.length; i++) {
			const x = xs[i],
				y = ys[i];
			if (x == null || y == null) continue;
			minX = Math.min(minX, x);
			maxX = Math.max(maxX, x);
			minY = Math.min(minY, y);
			maxY = Math.max(maxY, y);
		}
		return { minX, maxX, minY, maxY };
	});

	function zoomBy(factor: number) {
		zoom = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, zoom * factor));
	}

	/** Ctrl/⌘ + wheel zooms; the plain wheel keeps scrolling the page. */
	$effect(() => {
		if (!canvas) return;
		const el = canvas;
		const onWheel = (e: WheelEvent) => {
			if (!e.ctrlKey && !e.metaKey) return;
			e.preventDefault();
			zoomBy(Math.exp(-e.deltaY * 0.002));
		};
		el.addEventListener('wheel', onWheel, { passive: false });
		return () => el.removeEventListener('wheel', onWheel);
	});

	function token(name: string, fallback: string): string {
		const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
		return v || fallback;
	}

	function polyline(ctx: CanvasRenderingContext2D, xs: (number | null)[], ys: (number | null)[]) {
		ctx.beginPath();
		let pen = false;
		for (let k = 0; k < xs.length; k++) {
			const x = xs[k],
				y = ys[k];
			if (x == null || y == null) {
				pen = false;
				continue;
			}
			if (pen) ctx.lineTo(x, y);
			else ctx.moveTo(x, y);
			pen = true;
		}
	}

	/** A car: an arrow in the driving direction with a ring in the surface colour. */
	function car(
		ctx: CanvasRenderingContext2D,
		p: Point,
		heading: number,
		color: string,
		ring: string,
		px: number
	) {
		ctx.save();
		ctx.translate(p.x, p.y);
		ctx.rotate(heading - Math.PI / 2);
		ctx.beginPath();
		ctx.moveTo(0, 10 * px);
		ctx.lineTo(7 * px, -7 * px);
		ctx.lineTo(0, -3 * px);
		ctx.lineTo(-7 * px, -7 * px);
		ctx.closePath();
		ctx.lineJoin = 'round';
		ctx.lineWidth = 3 * px;
		ctx.strokeStyle = ring;
		ctx.stroke();
		ctx.fillStyle = color;
		ctx.fill();
		ctx.restore();
	}

	$effect(() => {
		if (!canvas || !hasMap || width === 0) return;
		const ctx = canvas.getContext('2d');
		if (!ctx) return;
		const dpr = window.devicePixelRatio || 1;
		canvas.width = width * dpr;
		canvas.height = HEIGHT * dpr;
		const surface = token('--color-zinc-900', '#212125');
		const page = token('--color-zinc-950', '#161619');
		const muted = token('--color-zinc-400', '#9f9fa9');
		const grid = token('--color-zinc-800', '#2e2e33');

		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ctx.fillStyle = surface;
		ctx.fillRect(0, 0, width, HEIGHT);

		const ref = data.laps[0];
		const centre = positionAt(ref, cursor);
		if (!centre) return;
		const heading = headingUp
			? (headingAt(ref, cursor, data.stepM, 8) ?? Math.PI / 2)
			: Math.PI / 2;
		const rotation = Math.PI / 2 - heading;
		const px = 1 / zoom; // one screen pixel in metres

		// world (x east, y north) → screen: centre on the reference car
		ctx.save();
		ctx.translate(width / 2, HEIGHT / 2);
		ctx.scale(zoom, -zoom);
		ctx.rotate(rotation);
		ctx.translate(-centre.x, -centre.y);

		// lines: the others first, the reference on top
		ctx.lineCap = 'round';
		ctx.lineJoin = 'round';
		for (let k = data.laps.length - 1; k >= 0; k--) {
			const lap = data.laps[k];
			if (!lap.channels.pos_x || !lap.channels.pos_y) continue;
			polyline(ctx, lap.channels.pos_x, lap.channels.pos_y);
			ctx.strokeStyle = lapColor(k);
			ctx.globalAlpha = k === 0 ? 0.9 : 0.75;
			ctx.lineWidth = (k === 0 ? 2.5 : 2) * px;
			ctx.stroke();
		}
		ctx.globalAlpha = 1;

		// start/finish line across the reference line
		const start = firstIndex(data);
		const s = positionAt(ref, start);
		const sh = headingAt(ref, start, data.stepM, 8);
		if (s && sh !== null) {
			const nx = Math.cos(sh + Math.PI / 2) * 8;
			const ny = Math.sin(sh + Math.PI / 2) * 8;
			ctx.beginPath();
			ctx.moveTo(s.x - nx, s.y - ny);
			ctx.lineTo(s.x + nx, s.y + ny);
			ctx.strokeStyle = muted;
			ctx.lineWidth = 2 * px;
			ctx.stroke();
		}

		// cars: the reference first, the others on top of it
		data.laps.forEach((lap, k) => {
			const i = carIndex(data, k, cursor, mode);
			if (i === null) return;
			const p = positionAt(lap, i);
			const h = headingAt(lap, i, data.stepM);
			if (p && h !== null) car(ctx, p, h, lapColor(k), surface, px);
		});
		ctx.restore();

		// overview of the whole track, bottom right, with the visible area
		const ow = Math.min(200, width * 0.3);
		const oh = 150;
		const ox = width - ow - 12;
		const oy = HEIGHT - oh - 12;
		const { minX, maxX, minY, maxY } = bounds;
		if (Number.isFinite(minX) && maxX > minX && maxY > minY) {
			ctx.fillStyle = page;
			ctx.globalAlpha = 0.85;
			ctx.fillRect(ox, oy, ow, oh);
			ctx.globalAlpha = 1;
			ctx.strokeStyle = grid;
			ctx.lineWidth = 1;
			ctx.strokeRect(ox + 0.5, oy + 0.5, ow - 1, oh - 1);
			const pad = 10;
			const os = Math.min((ow - 2 * pad) / (maxX - minX), (oh - 2 * pad) / (maxY - minY));
			const ocx = ox + ow / 2;
			const ocy = oy + oh / 2;
			const mx = (minX + maxX) / 2;
			const my = (minY + maxY) / 2;
			const toO = (p: Point) => ({ x: ocx + (p.x - mx) * os, y: ocy - (p.y - my) * os });
			ctx.save();
			ctx.translate(ocx, ocy);
			ctx.scale(os, -os);
			ctx.translate(-mx, -my);
			polyline(ctx, ref.channels.pos_x ?? [], ref.channels.pos_y ?? []);
			ctx.strokeStyle = muted;
			ctx.lineWidth = 1.5 / os;
			ctx.stroke();
			ctx.restore();
			// visible area: the screen corners back in world coordinates
			const cos = Math.cos(-rotation);
			const sin = Math.sin(-rotation);
			const corner = (sx: number, sy: number) => {
				const dx = (sx - width / 2) / zoom;
				const dy = -(sy - HEIGHT / 2) / zoom;
				return toO({
					x: centre.x + dx * cos - dy * sin,
					y: centre.y + dx * sin + dy * cos
				});
			};
			const corners = [corner(0, 0), corner(width, 0), corner(width, HEIGHT), corner(0, HEIGHT)];
			ctx.save();
			ctx.beginPath();
			ctx.rect(ox, oy, ow, oh);
			ctx.clip();
			ctx.beginPath();
			corners.forEach((c, i) => (i ? ctx.lineTo(c.x, c.y) : ctx.moveTo(c.x, c.y)));
			ctx.closePath();
			ctx.strokeStyle = lapColor(0);
			ctx.globalAlpha = 0.6;
			ctx.lineWidth = 1;
			ctx.stroke();
			ctx.globalAlpha = 1;
			// the cars as dots
			for (let k = data.laps.length - 1; k >= 0; k--) {
				const i = carIndex(data, k, cursor, mode);
				const p = i === null ? null : positionAt(data.laps[k], i);
				if (!p) continue;
				const o = toO(p);
				ctx.beginPath();
				ctx.arc(o.x, o.y, 3.5, 0, Math.PI * 2);
				ctx.fillStyle = lapColor(k);
				ctx.fill();
			}
			ctx.restore();
		}
	});

	const control =
		'rounded px-2 py-1 text-xs text-zinc-300 hover:bg-zinc-800 hover:text-zinc-100 aria-pressed:bg-zinc-700 aria-pressed:text-zinc-100';
</script>

<div bind:clientWidth={width} class="relative overflow-hidden rounded border border-zinc-800">
	{#if hasMap}
		<canvas
			bind:this={canvas}
			class="block"
			style:width="{width}px"
			style:height="{HEIGHT}px"
			aria-label="Track map following the reference lap"
		></canvas>

		<!-- the gap of every lap to the reference -->
		<ul
			class="absolute top-3 left-3 rounded border border-zinc-800 bg-zinc-950/85 px-3 py-2 text-xs"
			aria-label="Gaps to the reference"
		>
			{#each data.laps as lap, k (lap.lapId)}
				<li class="flex items-center gap-2 py-0.5">
					<span class="inline-block h-0.5 w-4 rounded" style:background={lapColor(k)}></span>
					<span class="text-zinc-400">{labels[k]}</span>
					<span class="ml-auto pl-3 font-mono text-zinc-100 tabular-nums"
						>{gapText(data, k, cursor, mode)}</span
					>
				</li>
			{/each}
		</ul>

		<!-- view controls -->
		<div
			class="absolute top-3 right-3 flex flex-col items-end gap-1.5 text-xs"
			role="toolbar"
			aria-label="Map"
		>
			<div class="flex rounded border border-zinc-800 bg-zinc-950/85 p-0.5" role="group">
				<span class="px-2 py-1 text-zinc-500">Cars by</span>
				<button
					type="button"
					class={control}
					aria-pressed={mode === 'distance'}
					title="Every car at the same distance as the reference: compare lines"
					onclick={() => (mode = 'distance')}>Distance</button
				>
				<button
					type="button"
					class={control}
					aria-pressed={mode === 'time'}
					title="Every car where it was after the same lap time: see the gaps"
					onclick={() => (mode = 'time')}>Time</button
				>
			</div>
			<div class="flex rounded border border-zinc-800 bg-zinc-950/85 p-0.5" role="group">
				<button
					type="button"
					class={control}
					aria-pressed={!headingUp}
					onclick={() => (headingUp = false)}>North up</button
				>
				<button
					type="button"
					class={control}
					aria-pressed={headingUp}
					onclick={() => (headingUp = true)}>Heading up</button
				>
			</div>
			<div class="flex rounded border border-zinc-800 bg-zinc-950/85 p-0.5" role="group">
				<button
					type="button"
					class={control}
					aria-label="Zoom out"
					disabled={zoom <= ZOOM_MIN}
					onclick={() => zoomBy(1 / 1.5)}>−</button
				>
				<button
					type="button"
					class={control}
					aria-label="Zoom in"
					disabled={zoom >= ZOOM_MAX}
					onclick={() => zoomBy(1.5)}>+</button
				>
			</div>
			<span class="text-zinc-500">Ctrl + scroll to zoom</span>
		</div>
	{:else}
		<p class="p-4 text-sm text-zinc-500">No position data for a track map.</p>
	{/if}
</div>
