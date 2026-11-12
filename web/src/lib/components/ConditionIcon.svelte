<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- Line icons for the lap table's condition columns (24×24, current colour). -->
<script lang="ts">
	import type { ConditionKey } from '#lib/lap-conditions.ts';

	let { name, class: cls = 'h-4 w-4' }: { name: ConditionKey; class?: string } = $props();

	/** Thermometer on the left half */
	const thermometer = 'M5 4.5a2 2 0 0 1 4 0v9.3a3.5 3.5 0 1 1-4 0z M7 10v6';
</script>

<svg
	viewBox="0 0 24 24"
	fill="none"
	stroke="currentColor"
	stroke-width="1.75"
	stroke-linecap="round"
	stroke-linejoin="round"
	class={cls}
	aria-hidden="true"
>
	{#if name === 'airTemp'}
		<!-- thermometer and wind -->
		<path d={thermometer} />
		<path d="M13 7h5a2 2 0 1 0-2-2 M13 11h8 M13 15h4a2 2 0 1 1-2 2" />
	{:else if name === 'trackTemp'}
		<!-- thermometer and a road -->
		<path d={thermometer} />
		<path d="M13 21l2.5-16 M23 21l-2.5-16 M18 7v1.5 M18 12v1.5 M18 17v2" />
	{:else if name === 'humidity'}
		<path d="M12 3.5c3.5 4.5 6 7.6 6 10.8a6 6 0 0 1-12 0c0-3.2 2.5-6.3 6-10.8z" />
		<path d="M9.5 14.5a2.5 2.5 0 0 0 2.5 2.5" />
	{:else if name === 'weather'}
		<!-- sun behind a cloud -->
		<circle cx="8" cy="8" r="3" />
		<path d="M8 2v1 M2 8h1 M3.8 3.8l.7.7 M12.2 3.8l-.7.7" />
		<path d="M10 20h8a3.5 3.5 0 0 0 0-7 5 5 0 0 0-9.6 1.4A2.8 2.8 0 0 0 10 20z" />
	{:else if name === 'fuel'}
		<!-- fuel pump -->
		<path d="M4 20V5a2 2 0 0 1 2-2h5a2 2 0 0 1 2 2v15 M3 20h11 M6.5 7h4" />
		<path d="M13 10h2a2 2 0 0 1 2 2v4a1.5 1.5 0 0 0 3 0V8l-3-3" />
	{:else if name === 'tyreTemp'}
		<!-- tyre and heat -->
		<circle cx="10" cy="14" r="7" />
		<circle cx="10" cy="14" r="2.5" />
		<path d="M17 2.5c-1 1.2 1 2.3 0 3.5 M20.5 2.5c-1 1.2 1 2.3 0 3.5" />
	{:else if name === 'tyreWear'}
		<!-- tyre with tread left on one side only -->
		<circle cx="12" cy="12" r="8.5" />
		<circle cx="12" cy="12" r="2.5" />
		<path
			d="M16.25 4.64l-1.25 2.17 M19.36 7.75l-2.17 1.25 M20.5 12H18 M19.36 16.25l-2.17-1.25 M16.25 19.36l-1.25-2.17"
		/>
	{/if}
</svg>
