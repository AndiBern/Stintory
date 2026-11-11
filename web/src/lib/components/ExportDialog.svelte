<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts" module>
	import type { ExportSelection } from '#lib/api/index.ts';

	/** One thing the user can choose to export. */
	export interface ExportScope {
		label: string;
		selection: ExportSelection;
		/** Laps for a CSV export; CSV is offered only if set */
		csvLapIds?: string[];
		/** File name without extension */
		fileName: string;
	}
</script>

<script lang="ts">
	import { api } from '#lib/api/index.ts';
	import { app, errorText } from '#lib/app-state.svelte.ts';

	let {
		open = $bindable(false),
		scopes
	}: {
		open?: boolean;
		/** First one is preselected */
		scopes: ExportScope[];
	} = $props();

	const client = api();
	let dialog: HTMLDialogElement | undefined = $state();
	let scopeIndex = $state(0);
	let format = $state<'bundle' | 'csv'>('bundle');
	let driverName = $state('');
	let includeSetup = $state(false);
	let busy = $state(false);

	const scope = $derived(scopes[Math.min(scopeIndex, scopes.length - 1)]);
	const csvPossible = $derived(!!scope?.csvLapIds?.length);

	$effect(() => {
		if (!dialog) return;
		if (open && !dialog.open) {
			scopeIndex = 0;
			busy = false;
			dialog.showModal();
			client
				.getSettings()
				.then((s) => (driverName = s.exportDriverName ?? ''))
				.catch(() => {});
		} else if (!open && dialog.open) {
			dialog.close();
		}
	});

	$effect(() => {
		if (!csvPossible) format = 'bundle';
	});

	async function run(event: SubmitEvent) {
		event.preventDefault();
		if (!scope || busy) return;
		busy = true;
		try {
			const result =
				format === 'csv'
					? await client.exportCsv(scope.csvLapIds!, scope.fileName)
					: await client.exportBundle(
							{
								...scope.selection,
								driverName: driverName.trim() || null,
								includeSetup
							},
							scope.fileName
						);
			if (!result) return; // file dialog cancelled: stay open
			const laps = result.laps === 1 ? '1 lap' : `${result.laps} laps`;
			app.notice = { tone: 'ok', text: `Exported ${laps} to ${result.path}` };
			open = false;
		} catch (e) {
			app.notice = { tone: 'error', text: `Export failed: ${errorText(e)}` };
			open = false;
		} finally {
			busy = false;
		}
	}
</script>

<dialog
	bind:this={dialog}
	onclose={() => (open = false)}
	class="m-auto w-[28rem] max-w-[calc(100vw-2rem)] rounded border border-zinc-700 bg-zinc-900 p-0 text-zinc-100 backdrop:bg-black/60"
	aria-labelledby="export-title"
>
	<form class="flex flex-col gap-4 p-5" onsubmit={run}>
		<h2 id="export-title" class="text-lg font-semibold">Export</h2>

		{#if scopes.length > 1}
			<fieldset class="flex flex-col gap-1 text-sm">
				<legend class="mb-1 text-zinc-400">What</legend>
				{#each scopes as s, i (s.label)}
					<label class="flex items-center gap-2">
						<input type="radio" name="scope" value={i} bind:group={scopeIndex} />
						{s.label}
					</label>
				{/each}
			</fieldset>
		{:else if scope}
			<p class="text-sm text-zinc-300">{scope.label}</p>
		{/if}

		<fieldset class="flex flex-col gap-1 text-sm">
			<legend class="mb-1 text-zinc-400">Format</legend>
			<label class="flex items-start gap-2">
				<input type="radio" name="format" value="bundle" bind:group={format} class="mt-1" />
				<span>
					Stintory file (.stintory)
					<span class="block text-xs text-zinc-400"
						>All data, for another Stintory installation or to share with someone.</span
					>
				</span>
			</label>
			<label class="flex items-start gap-2" class:opacity-40={!csvPossible}>
				<input
					type="radio"
					name="format"
					value="csv"
					bind:group={format}
					disabled={!csvPossible}
					class="mt-1"
				/>
				<span>
					CSV, one file per lap
					<span class="block text-xs text-zinc-400"
						>Telemetry only, for spreadsheets and other tools.</span
					>
				</span>
			</label>
		</fieldset>

		{#if format === 'bundle'}
			<label class="flex flex-col gap-1 text-sm">
				<span class="text-zinc-400">Driver name shown to others (optional)</span>
				<input
					type="text"
					bind:value={driverName}
					maxlength="64"
					placeholder="Anonymous"
					class="rounded border border-zinc-700 bg-zinc-950 px-2 py-1"
				/>
			</label>
			<label class="flex items-start gap-2 text-sm">
				<input type="checkbox" bind:checked={includeSetup} class="mt-1" />
				<span>
					Include car setups
					<span class="block text-xs text-zinc-400"
						>Off by default. Your game account name and id are never exported.</span
					>
				</span>
			</label>
		{/if}

		<div class="flex justify-end gap-2">
			<button
				type="button"
				class="rounded border border-zinc-700 px-3 py-1.5 text-sm hover:bg-zinc-800"
				onclick={() => (open = false)}>Cancel</button
			>
			<button
				type="submit"
				class="rounded bg-zinc-100 px-3 py-1.5 text-sm font-medium text-zinc-900 hover:bg-white disabled:opacity-50"
				disabled={busy || !scope}
			>
				{busy ? 'Exporting…' : 'Export…'}
			</button>
		</div>
	</form>
</dialog>
