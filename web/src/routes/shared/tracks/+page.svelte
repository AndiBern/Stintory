<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { api, type ReferenceLap, type SharedTrack } from '#lib/api/index.ts';
	import { app, errorText, sharingAvailable } from '#lib/app-state.svelte.ts';
	import ReferenceLapTable, { isShared } from '#lib/components/ReferenceLapTable.svelte';
	import SharingTabs from '#lib/components/SharingTabs.svelte';
	import { formatDelta, formatGame, formatLapTime } from '#lib/format.ts';

	const MAX_COMPARE = 8;
	const client = api();

	/** One track and car is shown when both are in the address */
	const trackKey = $derived(page.url.searchParams.get('track'));
	const carKey = $derived(page.url.searchParams.get('car'));

	let tracks = $state<SharedTrack[] | null>(null);
	let laps = $state<ReferenceLap[] | null>(null);
	let selected = $state<string[]>([]);
	let error = $state<string | null>(null);
	let busy = $state(false);
	let filter = $state('');

	$effect(() => {
		void app.sessionsVersion;
		if (!sharingAvailable()) return;
		const [track, car] = [trackKey, carKey];
		error = null;
		if (track && car) {
			laps = null;
			client
				.referenceLaps(track, car)
				.then((l) => {
					laps = l;
					selected = defaultSelection(l);
				})
				.catch((e) => (error = errorText(e)));
		} else {
			client.server
				.sharedTracks()
				.then((t) => (tracks = t))
				.catch((e) => (error = errorText(e)));
		}
	});

	/** Own best against the fastest shared lap: the usual question. */
	function defaultSelection(l: ReferenceLap[]): string[] {
		const shared = l.find(isShared);
		const own = l.find((x) => !isShared(x));
		return [shared, own].filter((x) => x !== undefined).map((x) => x.lapId);
	}

	const visibleTracks = $derived.by(() => {
		const q = filter.trim().toLowerCase();
		if (!tracks || !q) return tracks ?? [];
		return tracks.filter((t) =>
			[t.trackName, t.carName, t.bestDriver?.displayName ?? '', t.bestDriver?.username ?? ''].some(
				(v) => v.toLowerCase().includes(q)
			)
		);
	});

	const heading = $derived(laps?.[0] ? `${laps[0].trackName} · ${laps[0].carName}` : null);

	function open(t: SharedTrack) {
		const q = new URLSearchParams({ track: t.trackKey, car: t.carKey });
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- resolved path + query
		goto(`${resolve('/shared/tracks')}?${q}`);
	}

	/** Reference = fastest selected lap. */
	async function compare() {
		if (!laps || selected.length === 0) return;
		const chosen = laps.filter((l) => selected.includes(l.lapId));
		busy = true;
		try {
			await client.prepareLaps(chosen);
			const owner = chosen.find((l) => l.owner)?.owner?.id;
			const query = owner ? `?owner=${encodeURIComponent(owner)}` : '';
			const ids = chosen.map((l) => l.lapId).join(',');
			// eslint-disable-next-line svelte/no-navigation-without-resolve -- resolved path + query
			goto(resolve('/compare/[laps]', { laps: ids }) + query);
		} catch (e) {
			error = errorText(e);
		} finally {
			busy = false;
		}
	}
</script>

<h1 class="mb-4 text-2xl font-semibold">Sharing</h1>

{#if !sharingAvailable()}
	<p class="max-w-xl text-zinc-400">
		Sharing works through a Stintory server. Connect this app to a server under
		<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- resolved path + query -->
		<a class="underline" href={`${resolve('/settings')}?tab=sync`}>Settings</a> to see what others share
		with you.
	</p>
{:else}
	<SharingTabs />

	{#if error}
		<p class="mb-4 text-sm text-red-400" role="alert">{error}</p>
	{/if}

	{#if trackKey && carKey}
		<a href={resolve('/shared/tracks')} class="text-sm text-zinc-400 hover:text-zinc-200">
			← All tracks
		</a>
		<div class="mt-2 mb-3 flex flex-wrap items-start justify-between gap-4">
			<div>
				<h2 class="text-xl font-semibold">{heading ?? 'Reference laps'}</h2>
				<p class="text-sm text-zinc-400">
					The fastest laps here: yours and those others share with you. Select laps and compare
					them; the fastest one is the reference.
				</p>
			</div>
			<button
				type="button"
				class="rounded bg-zinc-100 px-3 py-1.5 text-sm font-medium text-zinc-900 hover:bg-white disabled:opacity-40"
				disabled={selected.length === 0 || busy}
				onclick={compare}
			>
				{busy ? 'Loading…' : selected.length > 1 ? `Compare ${selected.length} laps` : 'Show lap'}
			</button>
		</div>
		{#if laps}
			<ReferenceLapTable {laps} bind:selected max={MAX_COMPARE} />
		{:else if !error}
			<p class="text-sm text-zinc-400">Loading…</p>
		{/if}
	{:else}
		<div class="mb-2 flex flex-wrap items-center justify-between gap-2">
			<p class="text-sm text-zinc-400">
				Tracks and cars with laps others share with you, and how your best lap compares.
			</p>
			{#if tracks && tracks.length > 0}
				<input
					type="search"
					class="rounded border border-zinc-700 bg-zinc-900 px-3 py-1 text-sm"
					placeholder="Filter by track, car or driver"
					aria-label="Filter tracks"
					bind:value={filter}
				/>
			{/if}
		</div>
		{#if tracks === null}
			{#if !error}<p class="text-sm text-zinc-400">Loading…</p>{/if}
		{:else if tracks.length === 0}
			<p class="rounded border border-dashed border-zinc-700 p-6 text-center text-sm text-zinc-400">
				Nobody shares laps with you yet. Others can share with you directly, with a
				<a class="underline" href={resolve('/teams')}>team</a> you are in, or with everyone on this server.
			</p>
		{:else}
			<table class="w-full text-left text-sm">
				<thead class="text-zinc-400">
					<tr>
						<th class="py-2 font-normal">Track</th>
						<th class="font-normal">Car</th>
						<th class="text-right font-normal">Shared laps</th>
						<th class="text-right font-normal">Fastest</th>
						<th class="pl-4 font-normal">by</th>
						<th class="text-right font-normal">Your best</th>
						<th class="text-right font-normal">Gap</th>
					</tr>
				</thead>
				<tbody>
					{#each visibleTracks as t (t.trackKey + '|' + t.carKey)}
						<tr
							class="cursor-pointer border-t border-zinc-800 hover:bg-zinc-900"
							onclick={() => open(t)}
							onkeydown={(e) => e.key === 'Enter' && open(t)}
							tabindex="0"
						>
							<td class="py-2">
								<div>{t.trackName}</div>
								<div class="text-xs text-zinc-500">{formatGame(t.game)}</div>
							</td>
							<td>{t.carName}</td>
							<td class="text-right">
								{t.laps}
								<span class="text-xs text-zinc-500">
									· {t.drivers}
									{t.drivers === 1 ? 'driver' : 'drivers'}
								</span>
							</td>
							<td class="text-right font-mono">{formatLapTime(t.bestLapMs)}</td>
							<td class="pl-4">
								{#if t.bestDriver}
									{t.bestDriver.displayName || `@${t.bestDriver.username}`}
								{/if}
							</td>
							<td class="text-right font-mono">{formatLapTime(t.ownBestLapMs)}</td>
							<td class="text-right font-mono text-zinc-400">
								{#if t.ownBestLapMs !== null && t.bestLapMs !== null}
									{formatDelta(t.ownBestLapMs - t.bestLapMs)}
								{/if}
							</td>
						</tr>
					{:else}
						<tr><td colspan="7" class="py-3 text-zinc-400">Nothing matches the filter.</td></tr>
					{/each}
				</tbody>
			</table>
		{/if}
	{/if}
{/if}
