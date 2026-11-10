<!-- SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import type { CompareData } from '#lib/api/index.ts';
	import { lapColor } from './colors.ts';

	let {
		data,
		cursorIndex = null
	}: {
		data: CompareData;
		/** grid index to mark on every lap */
		cursorIndex?: number | null;
	} = $props();

	let canvas: HTMLCanvasElement | undefined = $state();
	let width = $state(300);

	const hasMap = $derived(data.laps.some((l) => l.channels.pos_x && l.channels.pos_y));

	/** Bounds over all laps (x east, y north). */
	const bounds = $derived.by(() => {
		let minX = Infinity,
			maxX = -Infinity,
			minY = Infinity,
			maxY = -Infinity;
		for (const lap of data.laps) {
			const xs = lap.channels.pos_x ?? [];
			const ys = lap.channels.pos_y ?? [];
			for (let i = 0; i < xs.length; i++) {
				const x = xs[i],
					y = ys[i];
				if (x === null || y === null) continue;
				minX = Math.min(minX, x);
				maxX = Math.max(maxX, x);
				minY = Math.min(minY, y);
				maxY = Math.max(maxY, y);
			}
		}
		return { minX, maxX, minY, maxY };
	});

	const height = $derived.by(() => {
		const w = bounds.maxX - bounds.minX;
		const h = bounds.maxY - bounds.minY;
		if (!(w > 0 && h > 0)) return width;
		return Math.min(Math.max((width * h) / w, 160), 600);
	});

	$effect(() => {
		if (!canvas || !hasMap) return;
		const ctx = canvas.getContext('2d');
		if (!ctx) return;
		const dpr = window.devicePixelRatio || 1;
		canvas.width = width * dpr;
		canvas.height = height * dpr;
		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ctx.clearRect(0, 0, width, height);

		const pad = 12;
		const { minX, maxX, minY, maxY } = bounds;
		const scale = Math.min((width - 2 * pad) / (maxX - minX), (height - 2 * pad) / (maxY - minY));
		const ox = (width - (maxX - minX) * scale) / 2;
		const oy = (height - (maxY - minY) * scale) / 2;
		const px = (x: number) => ox + (x - minX) * scale;
		const py = (y: number) => height - (oy + (y - minY) * scale); // north up

		// draw non-reference laps first, reference on top
		const order = data.laps.map((_, i) => i).reverse();
		for (const i of order) {
			const lap = data.laps[i];
			const xs = lap.channels.pos_x;
			const ys = lap.channels.pos_y;
			if (!xs || !ys) continue;
			ctx.beginPath();
			ctx.strokeStyle = lapColor(i);
			ctx.globalAlpha = i === 0 ? 0.9 : 0.6;
			ctx.lineWidth = i === 0 ? 3 : 1.5;
			let pen = false;
			for (let k = 0; k < xs.length; k++) {
				const x = xs[k],
					y = ys[k];
				if (x === null || y === null) {
					pen = false;
					continue;
				}
				if (pen) ctx.lineTo(px(x), py(y));
				else ctx.moveTo(px(x), py(y));
				pen = true;
			}
			ctx.stroke();
		}
		ctx.globalAlpha = 1;

		// start/finish marker
		const ref = data.laps[0];
		const sx = ref.channels.pos_x?.find((v) => v !== null);
		const syIdx = ref.channels.pos_x?.findIndex((v) => v !== null) ?? -1;
		const sy = syIdx >= 0 ? ref.channels.pos_y?.[syIdx] : null;
		if (sx != null && sy != null) {
			ctx.fillStyle = '#a1a1aa';
			ctx.fillRect(px(sx) - 3, py(sy) - 3, 6, 6);
		}

		// cursor position of each lap
		if (cursorIndex !== null) {
			data.laps.forEach((lap, i) => {
				const x = lap.channels.pos_x?.[cursorIndex];
				const y = lap.channels.pos_y?.[cursorIndex];
				if (x == null || y == null) return;
				ctx.beginPath();
				ctx.fillStyle = lapColor(i);
				ctx.strokeStyle = '#09090b';
				ctx.lineWidth = 2;
				ctx.arc(px(x), py(y), 5, 0, Math.PI * 2);
				ctx.fill();
				ctx.stroke();
			});
		}
	});
</script>

<div bind:clientWidth={width} class="w-full">
	{#if hasMap}
		<canvas
			bind:this={canvas}
			style:width="{width}px"
			style:height="{height}px"
			aria-label="Track map"
		></canvas>
	{:else}
		<p class="text-sm text-zinc-500">No position data for a track map.</p>
	{/if}
</div>
