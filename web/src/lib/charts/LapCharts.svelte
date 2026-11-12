<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import uPlot from 'uplot';
	import 'uplot/dist/uPlot.min.css';
	import type { CompareData } from '#lib/api/index.ts';
	import { lapColor } from './colors.ts';

	let {
		data,
		labels,
		onCursor
	}: {
		data: CompareData;
		/** legend label per lap (same order as data.laps) */
		labels: string[];
		/** called with the grid index under the cursor (null when leaving) */
		onCursor?: (index: number | null) => void;
	} = $props();

	let container: HTMLDivElement | undefined = $state();

	type Panel = {
		title: string;
		unit: string;
		height: number;
		/** one or more lines per lap: channel, transform, dashed, legend name */
		lines: { channel: string; scale: (v: number) => number; dash?: boolean; name?: string }[];
		delta?: boolean;
		stepped?: boolean;
	};

	const RAD_TO_DEG = 180 / Math.PI;

	function panels(d: CompareData): Panel[] {
		const has = (ch: string) => d.laps.some((l) => ch in l.channels);
		const list: Panel[] = [];
		if (d.laps.length > 1) {
			list.push({ title: 'Delta', unit: 's', height: 120, lines: [], delta: true });
		}
		if (has('speed'))
			list.push({
				title: 'Speed',
				unit: 'km/h',
				height: 170,
				lines: [{ channel: 'speed', scale: (v) => v * 3.6 }]
			});
		if (has('throttle'))
			list.push({
				title: 'Throttle',
				unit: '%',
				height: 110,
				lines: [{ channel: 'throttle', scale: (v) => v * 100 }]
			});
		if (has('brake'))
			list.push({
				title: 'Brake',
				unit: '%',
				height: 110,
				lines: [{ channel: 'brake', scale: (v) => v * 100 }]
			});
		if (has('steer'))
			list.push({
				title: 'Steering',
				unit: '°',
				height: 120,
				lines: [{ channel: 'steer', scale: (v) => v * RAD_TO_DEG }]
			});
		else if (has('steer_norm'))
			// games that report only the input (F1): -100 % full left … 100 % full right
			list.push({
				title: 'Steering',
				unit: '%',
				height: 120,
				lines: [{ channel: 'steer_norm', scale: (v) => v * 100 }]
			});
		if (has('gear'))
			list.push({
				title: 'Gear',
				unit: '',
				height: 90,
				stepped: true,
				lines: [{ channel: 'gear', scale: (v) => v }]
			});
		if (has('rpm'))
			list.push({
				title: 'RPM',
				unit: 'rpm',
				height: 110,
				lines: [{ channel: 'rpm', scale: (v) => v }]
			});
		// Le Mans Ultimate: virtual energy (solid) and battery (dashed)
		const energy = [
			{ channel: 'lmu.virtual_energy', scale: (v: number) => v * 100, name: 'VE' },
			{ channel: 'lmu.battery', scale: (v: number) => v * 100, dash: true, name: 'battery' }
		].filter((l) => has(l.channel));
		if (energy.length > 0)
			list.push({ title: 'Virtual energy / battery', unit: '%', height: 110, lines: energy });
		for (const [channel, title] of [
			['abs_active', 'ABS active'],
			['tc_active', 'TC active']
		] as const)
			if (has(channel))
				list.push({
					title,
					unit: '',
					height: 70,
					stepped: true,
					lines: [{ channel, scale: (v) => v }]
				});
		// driver aid settings (TC solid, ABS dashed) and the motor map
		const aids = [
			{ channel: 'lmu.tc_level', scale: (v: number) => v, name: 'TC' },
			{ channel: 'lmu.abs_level', scale: (v: number) => v, dash: true, name: 'ABS' }
		].filter((l) => has(l.channel));
		if (aids.length > 0)
			list.push({ title: 'TC / ABS level', unit: '', height: 90, stepped: true, lines: aids });
		if (has('lmu.motor_map'))
			list.push({
				title: 'Motor map',
				unit: '',
				height: 80,
				stepped: true,
				lines: [{ channel: 'lmu.motor_map', scale: (v) => v }]
			});
		return list;
	}

	function seriesData(
		d: CompareData,
		p: Panel
	): { values: (number | null)[][]; series: uPlot.Series[] } {
		const values: (number | null)[][] = [];
		const series: uPlot.Series[] = [{ label: 'm' }];
		d.laps.forEach((lap, i) => {
			if (p.delta) {
				if (i === 0) return; // reference vs. itself is 0
				values.push(lap.deltaS);
				series.push({ label: labels[i], stroke: lapColor(i), width: 1.5 });
				return;
			}
			for (const line of p.lines) {
				const src = lap.channels[line.channel];
				if (!src) continue;
				values.push(src.map((v) => (v === null ? null : line.scale(v))));
				series.push({
					label: `${labels[i]}${p.lines.length > 1 ? ` ${line.name ?? line.channel}` : ''}`,
					stroke: lapColor(i),
					width: i === 0 ? 1.25 : 1.5,
					dash: line.dash ? [4, 3] : undefined,
					paths: p.stepped ? uPlot.paths.stepped?.({ align: 1 }) : undefined
				});
			}
		});
		return { values, series };
	}

	$effect(() => {
		if (!container) return;
		const el = container;
		const d = data;
		const syncKey = `sy-${Math.random().toString(36).slice(2)}`;
		const charts: uPlot[] = [];
		let syncing = false;

		const axisStyle = {
			stroke: '#a1a1aa',
			grid: { stroke: '#2e2e33' },
			ticks: { stroke: '#424249' }
		};
		for (const p of panels(d)) {
			const { values, series } = seriesData(d, p);
			if (values.length === 0) continue;
			const div = document.createElement('div');
			div.className = 'mb-2';
			el.appendChild(div);
			const opts: uPlot.Options = {
				title: p.unit ? `${p.title} (${p.unit})` : p.title,
				width: el.clientWidth,
				height: p.height,
				series,
				legend: { show: false },
				scales: { x: { time: false } },
				axes: [
					{ ...axisStyle, values: (_u, ticks) => ticks.map((t) => `${(t / 1000).toFixed(1)} km`) },
					{ ...axisStyle, size: 50 }
				],
				cursor: {
					sync: { key: syncKey },
					drag: { x: true, y: false }
				},
				hooks: {
					setCursor: [(u) => onCursor?.(u.cursor.idx ?? null)],
					setScale: [
						(u, key) => {
							if (key !== 'x' || syncing) return;
							syncing = true;
							const { min, max } = u.scales.x;
							for (const c of charts) if (c !== u) c.setScale('x', { min: min!, max: max! });
							syncing = false;
						}
					]
				}
			};
			charts.push(new uPlot(opts, [d.distM, ...values] as uPlot.AlignedData, div));
		}

		const resize = new ResizeObserver(() => {
			for (const c of charts) c.setSize({ width: el.clientWidth, height: c.height });
		});
		resize.observe(el);
		const leave = () => onCursor?.(null);
		el.addEventListener('mouseleave', leave);

		return () => {
			resize.disconnect();
			el.removeEventListener('mouseleave', leave);
			for (const c of charts) c.destroy();
			el.replaceChildren();
		};
	});

	/** Double-click resets the zoom (uPlot default) – mention it in the UI. */
</script>

<div bind:this={container} class="lap-charts w-full"></div>

<style>
	.lap-charts :global(.u-title) {
		font-size: 0.8rem;
		font-weight: 500;
		color: #d4d4d8;
		text-align: left;
		padding-left: 52px;
	}
	.lap-charts :global(.u-select) {
		background: rgba(250, 250, 250, 0.08);
	}
</style>
