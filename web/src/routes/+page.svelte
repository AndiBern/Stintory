<!-- SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { api, type SessionSummary } from '#lib/api/index.ts';
	import { formatLapTime } from '#lib/format.ts';

	let sessions: Promise<SessionSummary[]> = $state(api().listSessions());
</script>

<h1 class="mb-4 text-2xl font-semibold">Sessions</h1>

{#await sessions}
	<p class="text-zinc-400">Loading…</p>
{:then list}
	{#if list.length === 0}
		<p class="text-zinc-400">
			No sessions yet. Start driving – sessions are recorded automatically.
		</p>
	{:else}
		<table class="w-full text-left text-sm">
			<thead class="text-zinc-400">
				<tr>
					<th class="py-2">Date</th>
					<th>Game</th>
					<th>Track</th>
					<th>Car</th>
					<th>Type</th>
					<th class="text-right">Laps</th>
					<th class="text-right">Best</th>
				</tr>
			</thead>
			<tbody>
				{#each list as s (s.id)}
					<tr class="border-t border-zinc-800">
						<td class="py-2">{new Date(s.startedAt).toLocaleString()}</td>
						<td>{s.game}</td>
						<td>{s.trackKey}</td>
						<td>{s.carKey}</td>
						<td>{s.sessionType}</td>
						<td class="text-right">{s.lapCount}</td>
						<td class="text-right font-mono">{formatLapTime(s.bestLapMs)}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
{:catch error}
	<p class="text-red-400" role="alert">Could not load sessions: {error.message}</p>
{/await}
