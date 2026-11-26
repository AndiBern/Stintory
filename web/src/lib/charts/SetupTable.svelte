<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!--
	The car setups of the compared laps side by side, so it shows whether the
	laps are comparable or the setup may make the difference. Only the
	settings that differ, unless asked for all.
-->
<script lang="ts">
	import type { LapTrace } from '#lib/api/index.ts';
	import { lapColor } from './colors.ts';
	import {
		differences,
		settingCount,
		setupGroups,
		setupStatus,
		setupSummary,
		type SetupStatus
	} from './setups.ts';
	import { loadFlag, saveFlag } from '#lib/view-prefs.ts';

	let {
		laps,
		labels
	}: {
		/** reference first */
		laps: LapTrace[];
		/** short label per lap */
		labels: string[];
	} = $props();

	let open = $state(loadFlag('compare.setup.open', true));
	let all = $state(false);

	function toggle() {
		open = !open;
		saveFlag('compare.setup.open', open);
	}

	const comparing = $derived(laps.length > 1);
	const statuses = $derived(laps.map(setupStatus));
	const known = $derived(statuses.filter((s) => s === 'lap' || s === 'session').length);
	const groups = $derived(setupGroups(laps));
	const diff = $derived(differences(groups));
	const diffCount = $derived(settingCount(diff));
	const total = $derived(settingCount(groups));
	/** a single lap, or all settings asked for */
	const shown = $derived(!comparing || all ? groups : diff);

	/** the labels of the laps with this status, e.g. `Lap 3, Max · L7` */
	function lapsWith(status: SetupStatus): string {
		return labels.filter((_, i) => statuses[i] === status).join(', ');
	}

	/** the session prescribed the setup */
	const fixed = $derived(laps.map((l) => !!l.fixedSetup));
	const fixedCount = $derived(fixed.filter(Boolean).length);

	const summary = $derived(setupSummary(laps));

	/** what the column says about a lap's setup */
	function note(i: number): string {
		return [fixed[i] ? 'fixed setup' : '', NOTE[statuses[i]]].filter(Boolean).join(' · ');
	}

	const NOTE: Record<SetupStatus, string> = {
		lap: '',
		session: 'setup of the session',
		hidden: 'not shared',
		none: 'no setup'
	};

	/** the value differs from the reference's */
	function changed(values: (string | null)[], i: number): boolean {
		return i > 0 && statuses[0] !== 'hidden' && statuses[0] !== 'none' && values[i] !== values[0];
	}
</script>

<section class="mb-5" aria-labelledby="setup-title">
	<div class="mb-2 flex flex-wrap items-center gap-x-4 gap-y-2">
		<h2 id="setup-title" class="text-lg font-semibold">
			<button
				type="button"
				class="flex items-center gap-1.5 hover:text-zinc-50"
				aria-expanded={open}
				aria-controls="setup-body"
				onclick={toggle}
			>
				<svg
					viewBox="0 0 24 24"
					class="h-4 w-4 text-zinc-400 transition-transform"
					class:-rotate-90={!open}
					fill="none"
					stroke="currentColor"
					stroke-width="2.5"
					stroke-linecap="round"
					stroke-linejoin="round"
					aria-hidden="true"><path d="m6 9 6 6 6-6" /></svg
				>Setup
			</button>
		</h2>
		<span
			class="text-xs"
			class:text-amber-300={comparing && diffCount > 0}
			class:text-zinc-400={!comparing || diffCount === 0}>{summary}</span
		>
		{#if open && comparing && known > 0 && total > 0}
			<button
				type="button"
				class="text-xs text-zinc-400 hover:text-zinc-200"
				aria-pressed={all}
				onclick={() => (all = !all)}
			>
				{all ? 'Only differences' : `Show all ${total} settings`}
			</button>
		{/if}
	</div>

	{#if open}
		<div id="setup-body">
			{#if shown.length > 0}
				<div class="overflow-x-auto">
					<table class="text-left text-sm">
						<thead class="text-zinc-400">
							<tr>
								<th class="py-2 pr-6 font-normal">Setting</th>
								{#each laps as l, i (l.lapId)}
									<th class="pr-6 font-normal whitespace-nowrap">
										<span class="flex items-center gap-1.5">
											<span class="inline-block h-0.5 w-3 rounded" style:background={lapColor(i)}
											></span>{labels[i]}
										</span>
										{#if note(i)}
											<span class="block text-xs text-zinc-500">{note(i)}</span>
										{/if}
									</th>
								{/each}
							</tr>
						</thead>
						<tbody>
							{#each shown as g (g.name)}
								<tr class="border-t border-zinc-800">
									<th
										colspan={laps.length + 1}
										scope="colgroup"
										class="pt-2 pb-1 text-xs font-medium tracking-wide text-zinc-400 uppercase"
										>{g.name}</th
									>
								</tr>
								{#each g.rows as r (r.name)}
									<tr class="hover:bg-zinc-900">
										<th scope="row" class="py-1 pr-6 font-normal text-zinc-300">{r.name}</th>
										{#each r.values as v, i (i)}
											<td
												class="pr-6 font-mono whitespace-nowrap tabular-nums"
												class:text-amber-300={changed(r.values, i)}
												class:text-zinc-200={!changed(r.values, i)}
											>
												{v ?? (statuses[i] === 'lap' || statuses[i] === 'session' ? '–' : '')}
											</td>
										{/each}
									</tr>
								{/each}
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
			<p class="mt-1 text-xs text-zinc-500">
				{#if comparing && diffCount > 0}
					Values that differ from the reference are highlighted.
				{/if}
				{#if fixedCount > 0}
					{fixedCount === laps.length
						? comparing
							? 'All laps'
							: 'This lap'
						: labels.filter((_, i) => fixed[i]).join(', ')}: a fixed setup, prescribed by the
					session.
				{/if}
				{#if statuses.includes('session')}
					{lapsWith('session')}: recorded before Stintory kept the setup of each lap, so this is the
					last setup of the session, which may have changed after the lap.
				{/if}
				{#if statuses.includes('hidden')}
					{lapsWith('hidden')}: the driver doesn’t share the setup with you.
				{/if}
				{#if statuses.includes('none')}
					{lapsWith('none')}: no setup. The game didn’t send one, or it was left out when the lap
					was exported or shared.
				{/if}
			</p>
		</div>
	{/if}
</section>
