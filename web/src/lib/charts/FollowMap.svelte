<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!--
	Track map that follows the reference car: it stays in the centre, the
	other laps' cars move around it (placed by distance or by time, see
	compare-view.ts). Each lap's line in its colour, an overview of the whole
	track in the corner. Split, every lap gets its own frame that follows its
	car; the controls and the overview stay in the first (the reference).
	The lines can show a telemetry value instead of the lap colour
	(map-colors.ts), with a legend in the first frame.
-->
<script lang="ts">
	import type { CompareData, Corner } from '#lib/api/index.ts';
	import { lapColor } from './colors.ts';
	import {
		carIndex,
		firstIndex,
		gapText,
		headingAt,
		positionAt,
		type OffsetMode,
		type Point,
		type Section
	} from './compare-view.ts';
	import { colorOptions, lineColoring, type ColorBy } from './map-colors.ts';

	let {
		data,
		cursor,
		labels,
		section = null,
		corners = [],
		mode = $bindable('distance')
	}: {
		data: CompareData;
		/** grid index of the reference car */
		cursor: number;
		/** short label per lap */
		labels: string[];
		/** the part of the lap being looked at, highlighted in the overview */
		section?: Section | null;
		/** corners, numbered at their apex on the outside of the bend */
		corners?: Corner[];
		mode?: OffsetMode;
	} = $props();

	const HEIGHT = 440;
	const ZOOM_MIN = 0.3;
	const ZOOM_MAX = 30;

	let canvases: (HTMLCanvasElement | undefined)[] = $state([]);
	let widths: number[] = $state([]);
	/** pixels per metre */
	let zoom = $state(3);
	let headingUp = $state(false);
	/** one frame per lap instead of all laps on one map */
	let splitWanted = $state(false);
	let colorWanted = $state<ColorBy>('lap');

	const colorChoices = $derived(colorOptions(data));
	/** back to the lap colour when the laps no longer have the chosen value */
	const colorBy = $derived(colorChoices.some((o) => o.key === colorWanted) ? colorWanted : 'lap');
	const coloring = $derived(lineColoring(data, colorBy));

	const hasMap = $derived(data.laps.some((l) => l.channels.pos_x && l.channels.pos_y));
	const split = $derived(splitWanted && data.laps.length > 1);

	/**
	 * What each frame shows: the lap whose car it follows and the laps drawn
	 * in it. The first frame has the controls and the overview.
	 */
	interface Frame {
		focus: number;
		laps: number[];
	}
	const frames: Frame[] = $derived(
		split
			? data.laps.map((_, k) => ({ focus: k, laps: [k] }))
			: [{ focus: 0, laps: data.laps.map((_, k) => k) }]
	);

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
		const els = canvases.slice(0, frames.length).filter((c) => c !== undefined);
		const onWheel = (e: WheelEvent) => {
			if (!e.ctrlKey && !e.metaKey) return;
			e.preventDefault();
			zoomBy(Math.exp(-e.deltaY * 0.002));
		};
		for (const el of els) el.addEventListener('wheel', onWheel, { passive: false });
		return () => {
			for (const el of els) el.removeEventListener('wheel', onWheel);
		};
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

	/**
	 * A line coloured by value: consecutive segments of the same bin in one
	 * stroke, only those near the visible area (`centre`, radius `r`).
	 */
	function coloredLine(
		ctx: CanvasRenderingContext2D,
		xs: (number | null)[],
		ys: (number | null)[],
		bins: Int16Array,
		palette: string[],
		centre: Point,
		r: number
	) {
		const r2 = r * r;
		const near = (x: number, y: number) => (x - centre.x) ** 2 + (y - centre.y) ** 2 < r2;
		let color = -1;
		let open = false;
		const flush = () => {
			if (!open) return;
			ctx.strokeStyle = palette[color];
			ctx.stroke();
			open = false;
		};
		for (let i = 0; i < xs.length - 1; i++) {
			const c = bins[i];
			const x0 = xs[i],
				y0 = ys[i],
				x1 = xs[i + 1],
				y1 = ys[i + 1];
			if (c < 0 || x0 == null || y0 == null || x1 == null || y1 == null) {
				flush();
				continue;
			}
			if (!near(x0, y0) && !near(x1, y1)) {
				flush();
				continue;
			}
			if (open && c === color) {
				ctx.lineTo(x1, y1);
				continue;
			}
			flush();
			ctx.beginPath();
			ctx.moveTo(x0, y0);
			ctx.lineTo(x1, y1);
			color = c;
			open = true;
		}
		flush();
	}

	interface Colors {
		surface: string;
		page: string;
		muted: string;
		grid: string;
		/** under lines coloured by value; laps without the value stay this grey */
		casing: string;
	}

	/**
	 * One frame: centred on the focus lap's car, with the lines and cars of
	 * `frame.laps`; the overview only where `overview` is set.
	 */
	function drawFrame(
		canvas: HTMLCanvasElement,
		width: number,
		frame: Frame,
		overview: boolean,
		{ surface, page, muted, grid, casing }: Colors
	) {
		const ctx = canvas.getContext('2d');
		if (!ctx) return;
		const dpr = window.devicePixelRatio || 1;
		canvas.width = width * dpr;
		canvas.height = HEIGHT * dpr;

		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ctx.fillStyle = surface;
		ctx.fillRect(0, 0, width, HEIGHT);

		const ref = data.laps[0];
		const focusLap = data.laps[frame.focus];
		const focusIndex = carIndex(data, frame.focus, cursor, mode);
		const centre = focusIndex === null ? null : positionAt(focusLap, focusIndex);
		if (focusIndex === null || !centre) {
			ctx.font = '12px ui-sans-serif, system-ui, sans-serif';
			ctx.textAlign = 'center';
			ctx.textBaseline = 'middle';
			ctx.fillStyle = muted;
			ctx.fillText('No position here', width / 2, HEIGHT / 2);
			return;
		}
		const heading = headingUp
			? (headingAt(focusLap, focusIndex, data.stepM, 8) ?? Math.PI / 2)
			: Math.PI / 2;
		const rotation = Math.PI / 2 - heading;
		const px = 1 / zoom; // one screen pixel in metres

		// world (x east, y north) → screen: centre on the focus car
		ctx.save();
		ctx.translate(width / 2, HEIGHT / 2);
		ctx.scale(zoom, -zoom);
		ctx.rotate(rotation);
		ctx.translate(-centre.x, -centre.y);

		// lines: the others first, the reference on top
		ctx.lineCap = 'round';
		ctx.lineJoin = 'round';
		// the visible area as a circle around the centre, for the coloured lines
		const reach = (Math.hypot(width, HEIGHT) / 2 + 20) / zoom;
		for (const k of [...frame.laps].reverse()) {
			const lap = data.laps[k];
			const xs = lap.channels.pos_x;
			const ys = lap.channels.pos_y;
			if (!xs || !ys) continue;
			polyline(ctx, xs, ys);
			if (!coloring) {
				ctx.strokeStyle = lapColor(k);
				ctx.globalAlpha = k === 0 ? 0.9 : 0.75;
				ctx.lineWidth = (k === 0 ? 2.5 : 2) * px;
				ctx.stroke();
				ctx.globalAlpha = 1;
				continue;
			}
			// coloured by value: wider in a frame of its own, on a grey casing
			// so that dark values (no throttle: black) stay visible; dashed for a
			// lap without the value
			const w = frame.laps.length === 1 ? 4 : k === 0 ? 3 : 2.5;
			const bins = coloring.bins[k];
			ctx.strokeStyle = casing;
			ctx.lineWidth = (w + 2) * px;
			if (!bins) {
				ctx.setLineDash([8 * px, 6 * px]);
				ctx.lineCap = 'butt';
			}
			ctx.stroke();
			ctx.setLineDash([]);
			ctx.lineCap = 'round';
			if (!bins) continue;
			ctx.lineWidth = w * px;
			coloredLine(ctx, xs, ys, bins, coloring.palette, centre, reach);
		}

		// start/finish line across the focus lap's line
		const start = firstIndex(data);
		const s = positionAt(focusLap, start);
		const sh = headingAt(focusLap, start, data.stepM, 8);
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

		// corner numbers, on the outside of the bend (before the cars, which
		// go on top)
		ctx.restore();
		ctx.font = '600 11px ui-sans-serif, system-ui, sans-serif';
		ctx.textAlign = 'center';
		ctx.textBaseline = 'middle';
		const rc = Math.cos(rotation);
		const rs = Math.sin(rotation);
		const toScreen = (p: Point) => {
			const dx = p.x - centre.x;
			const dy = p.y - centre.y;
			return {
				x: width / 2 + zoom * (dx * rc - dy * rs),
				y: HEIGHT / 2 - zoom * (dx * rs + dy * rc)
			};
		};
		for (const c of corners) {
			const i = Math.round(c.apexM / data.stepM);
			const p = positionAt(focusLap, i);
			const h = headingAt(focusLap, i, data.stepM, 8);
			if (!p || h === null) continue;
			// left of the driving direction is outside in a right-hander
			const out = h + (c.direction === 'right' ? Math.PI / 2 : -Math.PI / 2);
			const d = 22 / zoom;
			const s = toScreen({ x: p.x + Math.cos(out) * d, y: p.y + Math.sin(out) * d });
			if (s.x < -20 || s.y < -20 || s.x > width + 20 || s.y > HEIGHT + 20) continue;
			ctx.beginPath();
			ctx.arc(s.x, s.y, 10, 0, Math.PI * 2);
			ctx.fillStyle = page;
			ctx.globalAlpha = 0.85;
			ctx.fill();
			ctx.globalAlpha = 1;
			ctx.strokeStyle = grid;
			ctx.lineWidth = 1;
			ctx.stroke();
			ctx.fillStyle = muted;
			ctx.fillText(String(c.number), s.x, s.y + 0.5);
		}
		ctx.save();
		ctx.translate(width / 2, HEIGHT / 2);
		ctx.scale(zoom, -zoom);
		ctx.rotate(rotation);
		ctx.translate(-centre.x, -centre.y);

		// cars: the reference first, the others on top of it
		for (const k of frame.laps) {
			const lap = data.laps[k];
			const i = carIndex(data, k, cursor, mode);
			if (i === null) continue;
			const p = positionAt(lap, i);
			const h = headingAt(lap, i, data.stepM);
			if (p && h !== null) car(ctx, p, h, lapColor(k), surface, px);
		}
		ctx.restore();
		if (!overview) return;

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
			if (section) {
				const xs = ref.channels.pos_x ?? [];
				const ys = ref.channels.pos_y ?? [];
				polyline(
					ctx,
					xs.slice(section.start, section.end + 1),
					ys.slice(section.start, section.end + 1)
				);
				ctx.strokeStyle = lapColor(0);
				ctx.lineWidth = 3.5 / os;
				ctx.stroke();
			}
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
	}

	$effect(() => {
		if (!hasMap) return;
		const colors: Colors = {
			surface: token('--color-zinc-900', '#212125'),
			page: token('--color-zinc-950', '#161619'),
			muted: token('--color-zinc-400', '#9f9fa9'),
			grid: token('--color-zinc-800', '#2e2e33'),
			casing: token('--color-zinc-700', '#3f3f46')
		};
		frames.forEach((frame, f) => {
			const canvas = canvases[f];
			const width = widths[f] ?? 0;
			if (canvas && width > 0) drawFrame(canvas, width, frame, f === 0, colors);
		});
	});

	const control =
		'rounded px-2 py-1 text-xs text-zinc-300 hover:bg-zinc-800 hover:text-zinc-100 aria-pressed:bg-zinc-700 aria-pressed:text-zinc-100';
</script>

{#if hasMap}
	<div class={['grid gap-2', split && 'lg:grid-cols-2']}>
		{#each frames as frame, f (frame.focus)}
			<div
				bind:clientWidth={widths[f]}
				class="relative overflow-hidden rounded border border-zinc-800"
			>
				<canvas
					bind:this={canvases[f]}
					class="block"
					style:width="{widths[f] ?? 0}px"
					style:height="{HEIGHT}px"
					aria-label={split
						? `Track map following ${labels[frame.focus]}`
						: 'Track map following the reference lap'}
				></canvas>

				<!-- the gap of every lap in this frame to the reference -->
				<ul
					class="absolute top-3 left-3 rounded border border-zinc-800 bg-zinc-950/85 px-3 py-2 text-xs"
					aria-label="Gaps to the reference"
				>
					{#each frame.laps as k (data.laps[k].lapId)}
						<li class="flex items-center gap-2 py-0.5">
							<span class="inline-block h-0.5 w-4 rounded" style:background={lapColor(k)}></span>
							<span class="text-zinc-400">{labels[k]}</span>
							<span class="ml-auto pl-3 font-mono text-zinc-100 tabular-nums"
								>{gapText(data, k, cursor, mode)}</span
							>
						</li>
					{/each}
				</ul>

				{#if f === 0 && coloring}
					{@const legend = coloring.legend}
					<!-- what the line colours mean -->
					<div
						class="absolute bottom-3 left-3 w-56 max-w-[calc(100%-1.5rem)] rounded border border-zinc-800 bg-zinc-950/85 px-3 py-2 text-xs"
						role="group"
						aria-label="Line colours"
					>
						<p class="mb-1.5 text-zinc-400">{legend.title}</p>
						{#if legend.kind === 'ramp'}
							<div class="h-2 rounded-sm" style:background={legend.gradient}></div>
							<div class="mt-1 flex justify-between font-mono text-zinc-100 tabular-nums">
								<span>{legend.min}</span><span>{legend.max}</span>
							</div>
							{#if legend.minNote || legend.maxNote}
								<div class="flex justify-between text-zinc-500">
									<span>{legend.minNote}</span><span>{legend.maxNote}</span>
								</div>
							{/if}
						{:else}
							<div class="flex flex-wrap gap-x-2.5 gap-y-1">
								{#each legend.items as item (item.label)}
									<span class="flex items-center gap-1">
										<span class="inline-block h-2 w-3 rounded-sm" style:background={item.color}
										></span>
										<span class="text-zinc-300">{item.label}</span>
									</span>
								{/each}
							</div>
						{/if}
						{#if coloring.missing.length > 0}
							<p class="mt-1.5 text-amber-300">
								No data in {coloring.missing.map((k) => labels[k]).join(', ')} (older recording?), drawn
								grey dashed
							</p>
						{/if}
					</div>
				{/if}

				{#if f === 0}
					<!-- view controls, for all frames -->
					<div
						class="absolute top-3 right-3 flex flex-col items-end gap-1.5 text-xs"
						role="toolbar"
						aria-label="Map"
					>
						{#if data.laps.length > 1}
							<div class="flex rounded border border-zinc-800 bg-zinc-950/85 p-0.5" role="group">
								<span class="px-2 py-1 text-zinc-500">Laps</span>
								<button
									type="button"
									class={control}
									aria-pressed={!split}
									title="All laps on one map, following the reference"
									onclick={() => (splitWanted = false)}>One map</button
								>
								<button
									type="button"
									class={control}
									aria-pressed={split}
									title="Every lap in its own frame, following its car"
									onclick={() => (splitWanted = true)}>Own frames</button
								>
							</div>
						{/if}
						<label class="flex items-center rounded border border-zinc-800 bg-zinc-950/85 p-0.5">
							<span class="px-2 py-1 text-zinc-500">Colour</span>
							<select
								class="rounded bg-transparent px-1 py-1 text-xs text-zinc-100 hover:bg-zinc-800 focus:outline-none focus-visible:ring-1 focus-visible:ring-zinc-500"
								bind:value={() => colorBy, (v) => (colorWanted = v)}
							>
								{#each colorChoices as option (option.key)}
									<option class="bg-zinc-900" value={option.key}>{option.label}</option>
								{/each}
							</select>
						</label>
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
				{/if}
			</div>
		{/each}
	</div>
{:else}
	<p class="rounded border border-zinc-800 p-4 text-sm text-zinc-500">
		No position data for a track map.
	</p>
{/if}
