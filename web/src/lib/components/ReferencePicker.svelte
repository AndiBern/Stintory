<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts" module>
	/** The track and car to pick reference laps for. */
	export interface ReferenceTarget {
		trackKey: string;
		carKey: string;
		trackName: string;
		carName: string;
	}
</script>

<script lang="ts">
	import { api, type ReferenceLap } from '#lib/api/index.ts';
	import { errorText } from '#lib/app-state.svelte.ts';
	import ReferenceLapTable from './ReferenceLapTable.svelte';

	let {
		target = $bindable(null),
		exclude = [],
		max = 7,
		onpick
	}: {
		/** Set to open the dialog; null when closed */
		target: ReferenceTarget | null;
		/** Laps that are compared already */
		exclude?: string[];
		/** Most laps to add */
		max?: number;
		/** The chosen laps, fastest first, ready to compare */
		onpick: (laps: ReferenceLap[]) => void | Promise<void>;
	} = $props();

	const client = api();
	let dialog: HTMLDialogElement | undefined = $state();
	let laps = $state<ReferenceLap[] | null>(null);
	let selected = $state<string[]>([]);
	let error = $state<string | null>(null);
	let busy = $state(false);

	$effect(() => {
		if (!dialog) return;
		const t = target;
		if (t && !dialog.open) {
			laps = null;
			selected = [];
			error = null;
			busy = false;
			dialog.showModal();
			client
				.referenceLaps(t.trackKey, t.carKey)
				.then((l) => (laps = l))
				.catch((e) => (error = errorText(e)));
		} else if (!t && dialog.open) {
			dialog.close();
		}
	});

	async function add(event: SubmitEvent) {
		event.preventDefault();
		if (!laps || selected.length === 0 || busy) return;
		const chosen = laps.filter((l) => selected.includes(l.lapId));
		busy = true;
		error = null;
		try {
			await client.prepareLaps(chosen);
			await onpick(chosen);
			target = null;
		} catch (e) {
			error = errorText(e);
		} finally {
			busy = false;
		}
	}
</script>

<dialog
	bind:this={dialog}
	onclose={() => (target = null)}
	class="m-auto w-[48rem] max-w-[calc(100vw-2rem)] rounded border border-zinc-700 bg-zinc-900 p-0 text-zinc-100 backdrop:bg-black/60"
	aria-labelledby="reference-title"
>
	<form class="flex max-h-[80vh] flex-col gap-3 p-5" onsubmit={add}>
		<div>
			<h2 id="reference-title" class="text-lg font-semibold">Compare with…</h2>
			{#if target}
				<p class="text-sm text-zinc-400">
					Fastest laps of {target.trackName} · {target.carName}: yours and those shared with you.
				</p>
			{/if}
		</div>

		<div class="min-h-0 overflow-auto">
			{#if error}
				<p class="text-sm text-red-400" role="alert">{error}</p>
			{/if}
			{#if laps}
				<ReferenceLapTable {laps} bind:selected {exclude} {max} label="Laps to compare with" />
			{:else if !error}
				<p class="text-sm text-zinc-400">Loading…</p>
			{/if}
		</div>

		<div class="flex items-center justify-end gap-2">
			<button
				type="button"
				class="rounded border border-zinc-700 px-3 py-1.5 text-sm hover:bg-zinc-800"
				onclick={() => (target = null)}
			>
				Cancel
			</button>
			<button
				type="submit"
				class="rounded bg-zinc-100 px-3 py-1.5 text-sm font-medium text-zinc-900 hover:bg-white disabled:opacity-50"
				disabled={busy || selected.length === 0}
			>
				{busy
					? 'Loading…'
					: selected.length > 1
						? `Compare with ${selected.length} laps`
						: 'Compare'}
			</button>
		</div>
	</form>
</dialog>
