<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts" module>
	import type { ReferenceLap, ReferenceScope } from '#lib/api/index.ts';

	/** Laps someone else drove: shared on the server or kept copies of such laps. */
	export function isShared(l: ReferenceLap): boolean {
		return l.owner !== null || l.origin.kind === 'synced';
	}

	/** Laps of a scope ('mine' = recorded and imported). */
	export function inScope(laps: ReferenceLap[], scope: ReferenceScope): ReferenceLap[] {
		if (scope === 'all') return laps;
		return laps.filter((l) => isShared(l) === (scope === 'shared'));
	}
</script>

<script lang="ts">
	import {
		formatDateTime,
		formatDelta,
		formatDriver,
		formatLapTime,
		formatSessionType
	} from '#lib/format.ts';

	let {
		laps,
		selected = $bindable([]),
		max = 8,
		exclude = [],
		label = 'Reference laps'
	}: {
		/** Fastest first */
		laps: ReferenceLap[];
		/** Selected lap ids */
		selected?: string[];
		/** Most laps to select */
		max?: number;
		/** Laps that can't be chosen (e.g. already compared) */
		exclude?: string[];
		/** Accessible name of the table */
		label?: string;
	} = $props();

	let scope = $state<ReferenceScope>('all');

	const counts = $derived({
		all: laps.length,
		mine: inScope(laps, 'mine').length,
		shared: inScope(laps, 'shared').length
	});
	const visible = $derived(inScope(laps, scope));
	const fastest = $derived(laps[0]?.lapTimeMs);

	const scopes: { id: ReferenceScope; label: string }[] = [
		{ id: 'all', label: 'All' },
		{ id: 'mine', label: 'Mine' },
		{ id: 'shared', label: 'Shared with me' }
	];

	function toggle(id: string) {
		if (selected.includes(id)) selected = selected.filter((s) => s !== id);
		else if (selected.length < max) selected = [...selected, id];
	}
</script>

<div class="mb-2 flex gap-1 text-sm" role="group" aria-label="Show">
	{#each scopes as s (s.id)}
		<button
			type="button"
			class="rounded px-2 py-0.5 hover:bg-zinc-800"
			class:bg-zinc-800={scope === s.id}
			aria-pressed={scope === s.id}
			onclick={() => (scope = s.id)}
		>
			{s.label} <span class="text-zinc-500">{counts[s.id]}</span>
		</button>
	{/each}
</div>

{#if visible.length === 0}
	<p class="rounded border border-dashed border-zinc-700 p-4 text-center text-sm text-zinc-400">
		{#if scope === 'shared'}
			Nobody shares laps of this track and car with you yet.
		{:else if scope === 'mine'}
			You have no complete, valid laps of this track and car.
		{:else}
			No complete, valid laps of this track and car.
		{/if}
	</p>
{:else}
	<table class="w-full text-left text-sm" aria-label={label}>
		<thead class="text-zinc-400">
			<tr>
				<th class="w-8 py-2"><span class="sr-only">Select</span></th>
				<th class="text-right font-normal">Time</th>
				<th class="text-right font-normal">Δ fastest</th>
				<th class="pl-4 font-normal">Driver</th>
				<th class="font-normal">Lap</th>
				<th class="font-normal">Session</th>
			</tr>
		</thead>
		<tbody>
			{#each visible as l (l.lapId)}
				{@const taken = exclude.includes(l.lapId)}
				{@const checked = selected.includes(l.lapId)}
				<tr class="border-t border-zinc-800 hover:bg-zinc-900" class:opacity-50={taken}>
					<td class="py-1.5">
						<input
							type="checkbox"
							aria-label="Select {formatDriver(l)} lap {l.number} ({formatLapTime(l.lapTimeMs)})"
							checked={checked || taken}
							disabled={taken || (!checked && selected.length >= max)}
							onchange={() => toggle(l.lapId)}
						/>
					</td>
					<td class="text-right font-mono" class:text-purple-300={l.lapTimeMs === fastest}>
						{formatLapTime(l.lapTimeMs)}
					</td>
					<td class="text-right font-mono text-zinc-400">
						{#if fastest !== undefined && l.lapTimeMs !== fastest}
							{formatDelta(l.lapTimeMs - fastest)}
						{/if}
					</td>
					<td class="pl-4">
						<span class:text-zinc-400={!isShared(l) && l.origin.kind === 'own'}>
							{formatDriver(l)}
						</span>
						{#if l.owner && l.owner.displayName}
							<span class="text-xs text-zinc-500">@{l.owner.username}</span>
						{/if}
						{#if l.origin.kind === 'imported'}
							<span class="ml-1 rounded bg-zinc-800 px-1.5 text-xs text-zinc-300">imported</span>
						{/if}
						{#if l.remote}
							<span
								class="ml-1 rounded bg-sky-950 px-1.5 text-xs text-sky-300"
								title="Kept on this computer once you compare with it"
							>
								on the server
							</span>
						{/if}
					</td>
					<td>{l.number}</td>
					<td class="text-zinc-400">
						{formatSessionType(l.sessionType)} · {formatDateTime(l.sessionStartedAt)}
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
{/if}
