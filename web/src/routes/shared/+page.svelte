<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { api, type DefaultSharing, type Grantee, type SharedSession } from '#lib/api/index.ts';
	import { app, errorText, sharingAvailable } from '#lib/app-state.svelte.ts';
	import GranteePicker from '#lib/components/GranteePicker.svelte';
	import SharingTabs from '#lib/components/SharingTabs.svelte';
	import {
		formatAudience,
		formatDateTime,
		formatGame,
		formatLapTime,
		formatSessionType
	} from '#lib/format.ts';

	const server = api().server;

	let sessions = $state<SharedSession[] | null>(null);
	let defaults = $state<DefaultSharing | null>(null);
	let grants = $state<Grantee[]>([]);
	let error = $state<string | null>(null);
	let saved = $state<string | null>(null);
	let filter = $state('');

	async function load() {
		try {
			const [s, d] = await Promise.all([server.sharedSessions(), server.defaultSharing()]);
			sessions = s;
			defaults = d;
			grants = d.grants;
			error = null;
		} catch (e) {
			error = errorText(e);
		}
	}

	$effect(() => {
		void app.sessionsVersion;
		if (sharingAvailable()) void load();
	});

	const visible = $derived.by(() => {
		const q = filter.trim().toLowerCase();
		if (!sessions || !q) return sessions ?? [];
		return sessions.filter((s) =>
			[s.trackName, s.carName, s.owner.displayName, s.owner.username].some((v) =>
				v.toLowerCase().includes(q)
			)
		);
	});

	function open(s: SharedSession) {
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- resolved path + query
		goto(`${resolve('/session/[id]', { id: s.id })}?owner=${encodeURIComponent(s.owner.id)}`);
	}

	const changed = $derived(
		defaults !== null && JSON.stringify(key(grants)) !== JSON.stringify(key(defaults.grants))
	);

	function key(list: Grantee[]): string[] {
		return list.map((g) => (g.kind === 'everyone' ? g.kind : `${g.kind}:${g.id}`)).sort();
	}

	async function saveDefault(event: SubmitEvent) {
		event.preventDefault();
		if (!defaults) return;
		const audience = defaults.everyoneAllowed
			? grants
			: grants.filter((g) => g.kind !== 'everyone');
		const n = defaults.inheritingSessions;
		if (
			audience.length > 0 &&
			!confirm(
				`Share what you record with: ${formatAudience(audience)}?\n\nThis applies to ${n} session(s) that use your default now, and to everything you record from now on.`
			)
		)
			return;
		try {
			defaults = await server.setDefaultSharing(audience);
			grants = defaults.grants;
			saved = `Saved. Applies to ${defaults.inheritingSessions} session(s).`;
			error = null;
		} catch (e) {
			saved = null;
			error = errorText(e);
		}
	}
</script>

<h1 class="mb-4 text-2xl font-semibold">Sharing</h1>

{#if !sharingAvailable()}
	<p class="max-w-xl text-zinc-400">
		Sharing works through a Stintory server. Connect this app to a server under
		<a class="underline" href={resolve('/settings')}>Settings</a> to share laps and to see what others
		share with you.
	</p>
{:else}
	<SharingTabs />

	{#if error}
		<p class="mb-4 text-sm text-red-400" role="alert">{error}</p>
	{/if}

	<section class="mb-6">
		<div class="mb-2 flex flex-wrap items-center justify-between gap-2">
			<h2 class="font-medium">Shared with me</h2>
			{#if sessions && sessions.length > 0}
				<input
					type="search"
					class="rounded border border-zinc-700 bg-zinc-900 px-3 py-1 text-sm"
					placeholder="Filter by track, car or driver"
					aria-label="Filter shared sessions"
					bind:value={filter}
				/>
			{/if}
		</div>
		{#if sessions === null}
			{#if !error}<p class="text-sm text-zinc-400">Loading…</p>{/if}
		{:else if sessions.length === 0}
			<p class="rounded border border-dashed border-zinc-700 p-6 text-center text-sm text-zinc-400">
				Nobody shares sessions with you yet. Others can share with you directly, with a
				<a class="underline" href={resolve('/teams')}>team</a> you are in, or with everyone on this server.
			</p>
		{:else}
			<table class="w-full text-left text-sm">
				<thead class="text-zinc-400">
					<tr>
						<th class="py-2 font-normal">Date</th>
						<th class="font-normal">Driver</th>
						<th class="font-normal">Track</th>
						<th class="font-normal">Car</th>
						<th class="font-normal">Type</th>
						<th class="text-right font-normal">Laps</th>
						<th class="text-right font-normal">Best</th>
					</tr>
				</thead>
				<tbody>
					{#each visible as s (s.owner.id + s.id)}
						<tr
							class="cursor-pointer border-t border-zinc-800 hover:bg-zinc-900"
							onclick={() => open(s)}
							onkeydown={(e) => e.key === 'Enter' && open(s)}
							tabindex="0"
						>
							<td class="py-2 whitespace-nowrap">{formatDateTime(s.startedAt)}</td>
							<td>
								<div>{s.owner.displayName}</div>
								<div class="text-xs text-zinc-500">@{s.owner.username}</div>
							</td>
							<td>
								<div>{s.trackName}</div>
								<div class="text-xs text-zinc-500">{formatGame(s.game)}</div>
							</td>
							<td>{s.carName}</td>
							<td>{formatSessionType(s.sessionType)}</td>
							<td class="text-right">{s.lapCount}</td>
							<td class="text-right font-mono">{formatLapTime(s.bestLapMs)}</td>
						</tr>
					{:else}
						<tr><td colspan="7" class="py-3 text-zinc-400">Nothing matches the filter.</td></tr>
					{/each}
				</tbody>
			</table>
		{/if}
	</section>

	{#if defaults}
		<section class="max-w-2xl rounded border border-zinc-800 p-4">
			<h2 class="font-medium">What I share by default</h2>
			<p class="mt-1 mb-3 text-sm text-zinc-400">
				Who sees the sessions you record, unless you choose something else for a session or a lap
				("Share…"). Right now: <strong class="text-zinc-200"
					>{formatAudience(
						defaults.everyoneAllowed
							? defaults.grants
							: defaults.grants.filter((g) => g.kind !== 'everyone')
					)}</strong
				>.
			</p>
			<form class="flex flex-col gap-3" onsubmit={saveDefault}>
				<GranteePicker bind:grants everyoneAllowed={defaults.everyoneAllowed} />
				<p class="text-xs text-zinc-500">
					Applies to {defaults.inheritingSessions} session(s) that use your default, and to new recordings.
					Imported sessions are never shared by default. Car setups are not shared.
				</p>
				<div class="flex items-center gap-3">
					<button
						type="submit"
						class="rounded bg-zinc-100 px-3 py-1.5 text-sm font-medium text-zinc-900 hover:bg-white disabled:opacity-50"
						disabled={!changed}>Save</button
					>
					{#if saved && !changed}<span class="text-sm text-zinc-300" role="status">{saved}</span
						>{/if}
				</div>
			</form>
		</section>
	{/if}
{/if}
