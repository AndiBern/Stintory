<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { confirmAction } from '#lib/confirm.ts';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { api, type LapConditions, type LapSummary, type SessionDetail } from '#lib/api/index.ts';
	import { app, sharingAvailable } from '#lib/app-state.svelte.ts';
	import LapTimeChart from '#lib/charts/LapTimeChart.svelte';
	import { isClean } from '#lib/charts/lap-times.ts';
	import ConditionIcon from '#lib/components/ConditionIcon.svelte';
	import ExportDialog, { type ExportScope } from '#lib/components/ExportDialog.svelte';
	import {
		anyFlat,
		cellText,
		compoundColor,
		compoundDots,
		fuelUsedText,
		tireSummary,
		visibleColumns,
		wheelDetails
	} from '#lib/lap-conditions.ts';
	import ReferencePicker, { type ReferenceTarget } from '#lib/components/ReferencePicker.svelte';
	import ShareDialog, { type ShareTarget } from '#lib/components/ShareDialog.svelte';
	import {
		fileDate,
		fileName,
		formatDateTime,
		formatDelta,
		formatGame,
		formatLapTime,
		formatOrigin,
		formatSessionType
	} from '#lib/format.ts';

	const MAX_COMPARE = 8;
	const client = api();
	const id = $derived(page.params.id ?? '');
	/** Set when the session belongs to someone else who shared it */
	const owner = $derived(page.url.searchParams.get('owner'));

	let detail = $state<SessionDetail | null>(null);
	let error = $state<string | null>(null);
	let selected = $state<string[]>([]);
	let showAll = $state(false);
	let exporting = $state(false);
	let sharing = $state<ShareTarget | null>(null);
	let picking = $state<ReferenceTarget | null>(null);

	/** Someone else's session: nothing to share, nothing to delete on the server */
	const foreign = $derived(detail?.summary.origin.kind === 'synced');
	const canShare = $derived(!foreign && sharingAvailable());

	$effect(() => {
		void app.sessionsVersion;
		const sessionId = id;
		error = null;
		client
			.getSession(sessionId, owner ?? undefined)
			.then((d) => {
				const keepSelection = detail?.summary.id === d.summary.id;
				detail = d;
				if (!keepSelection) selected = bestLap(d) ? [bestLap(d)!.id] : [];
			})
			.catch((e) => (error = e.message));
	});

	function bestLap(d: SessionDetail): LapSummary | undefined {
		return d.laps
			.filter(isClean)
			.reduce<LapSummary | undefined>(
				(b, l) => (!b || l.lapTimeMs! < b.lapTimeMs! ? l : b),
				undefined
			);
	}

	const best = $derived(detail ? bestLap(detail) : undefined);
	/** for the compound colours, which differ per game */
	const game = $derived(detail?.summary.game ?? 'f1_25');
	const visibleLaps = $derived(detail ? detail.laps.filter((l) => showAll || !l.incomplete) : []);
	const hiddenCount = $derived(detail ? detail.laps.length - visibleLaps.length : 0);
	/** Weather, fuel and tyre columns: only those the game provides */
	const conditionColumns = $derived(visibleColumns(visibleLaps));

	/** The lap time chart needs at least two laps with a time. */
	const showChart = $derived(visibleLaps.filter((l) => l.lapTimeMs !== null).length > 1);

	function toggle(lapId: string) {
		if (selected.includes(lapId)) selected = selected.filter((s) => s !== lapId);
		else if (selected.length < MAX_COMPARE) selected = [...selected, lapId];
	}

	/** The selected laps, fastest first. */
	function selectedByTime(): string[] {
		const time = (lapId: string) =>
			detail?.laps.find((l) => l.id === lapId)?.lapTimeMs ?? Number.MAX_SAFE_INTEGER;
		return [...selected].sort((a, b) => time(a) - time(b));
	}

	function openCompare(lapIds: string[]) {
		const query = owner ? `?owner=${encodeURIComponent(owner)}` : '';
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- resolved path + query
		goto(resolve('/compare/[laps]', { laps: lapIds.join(',') }) + query);
	}

	/** Reference = fastest selected lap. */
	function compare() {
		openCompare(selectedByTime());
	}

	function pickReference() {
		if (!detail) return;
		const s = detail.summary;
		picking = {
			trackKey: s.trackKey,
			carKey: s.carKey,
			trackName: s.trackName,
			carName: s.carName
		};
	}

	async function remove() {
		if (!detail) return;
		const question = foreign
			? 'Remove the copy of this shared session from this computer? You can open it again from "Sharing" as long as it is shared with you.'
			: `Delete this session with ${detail.laps.length} laps? This cannot be undone.`;
		if (!(await confirmAction(question))) return;
		await client.deleteSession(detail.summary.id);
		goto(resolve(foreign ? '/shared' : '/'));
	}

	const exportScopes: ExportScope[] = $derived.by(() => {
		if (!detail) return [];
		const s = detail.summary;
		const base = fileName(s.trackName, s.carName, fileDate(s.startedAt));
		const scopes: ExportScope[] = [];
		if (selected.length > 0) {
			const numbers = detail.laps
				.filter((l) => selected.includes(l.id))
				.map((l) => l.number)
				.sort((a, b) => a - b);
			scopes.push({
				label:
					selected.length === 1
						? `Selected lap (${numbers[0]})`
						: `${selected.length} selected laps`,
				selection: { lapIds: [...selected] },
				csvLapIds: [...selected],
				fileName: selected.length === 1 ? `${base} lap ${numbers[0]}` : base
			});
		}
		scopes.push({
			label: `Whole session (${detail.laps.length} laps)`,
			selection: { sessionIds: [s.id] },
			csvLapIds: detail.laps.map((l) => l.id),
			fileName: base
		});
		return scopes;
	});

	function status(l: LapSummary): string[] {
		const tags: string[] = [];
		if (l.outLap) tags.push('out-lap');
		if (l.inLap) tags.push('in-lap');
		if (l.flashback) tags.push('flashback');
		return tags;
	}
</script>

{#if error}
	<p class="text-red-400" role="alert">Could not load session: {error}</p>
{:else if !detail}
	<p class="text-zinc-400">Loading…</p>
{:else}
	{@const s = detail.summary}
	<a href={resolve(foreign ? '/shared' : '/')} class="text-sm text-zinc-400 hover:text-zinc-200"
		>← {foreign ? 'Sharing' : 'Sessions'}</a
	>
	<div class="mt-2 mb-4 flex flex-wrap items-start justify-between gap-4">
		<div>
			<h1 class="text-2xl font-semibold">{s.trackName}</h1>
			<p class="text-zinc-300">
				{s.carName}
				{#if formatOrigin(s.origin)}
					<span class="ml-1 rounded bg-sky-950 px-1.5 text-xs text-sky-300"
						>{formatOrigin(s.origin)}</span
					>
				{/if}
			</p>
			<p class="mt-1 text-sm text-zinc-400">
				{formatGame(s.game)} · {formatSessionType(s.sessionType)} · {formatDateTime(s.startedAt)}
				{#if detail.conditions.airTempC !== null}
					· air {detail.conditions.airTempC.toFixed(1)} °C
				{/if}
				{#if detail.conditions.trackTempC !== null}
					· track {detail.conditions.trackTempC.toFixed(1)} °C
				{/if}
				{#if detail.conditions.weather}
					· {detail.conditions.weather}
				{/if}
			</p>
		</div>
		<div class="flex gap-2">
			<button
				type="button"
				class="rounded bg-zinc-100 px-3 py-1.5 text-sm font-medium text-zinc-900 hover:bg-white disabled:opacity-40"
				disabled={selected.length === 0}
				onclick={compare}
			>
				{selected.length > 1 ? `Compare ${selected.length} laps` : 'Show lap'}
			</button>
			<button
				type="button"
				class="rounded border border-zinc-700 px-3 py-1.5 text-sm text-zinc-300 hover:bg-zinc-800 disabled:opacity-40"
				title="Compare the selected laps with your other laps or laps shared with you"
				disabled={selected.length >= MAX_COMPARE}
				onclick={pickReference}
			>
				Compare with…
			</button>
			{#if client.capabilities.exportFiles}
				<button
					type="button"
					class="rounded border border-zinc-700 px-3 py-1.5 text-sm text-zinc-300 hover:bg-zinc-800"
					onclick={() => (exporting = true)}
				>
					Export…
				</button>
			{/if}
			{#if canShare}
				<button
					type="button"
					class="rounded border border-zinc-700 px-3 py-1.5 text-sm text-zinc-300 hover:bg-zinc-800"
					onclick={() => (sharing = { kind: 'session', id: s.id, label: s.trackName })}
				>
					Share…
				</button>
			{/if}
			<!-- a shared session on the server isn't the viewer's to delete; the desktop app keeps a copy -->
			{#if !foreign || client.capabilities.localSettings}
				<button
					type="button"
					class="rounded border border-zinc-700 px-3 py-1.5 text-sm text-zinc-300 hover:border-red-700 hover:text-red-300"
					onclick={remove}
				>
					{foreign ? 'Remove copy' : 'Delete session'}
				</button>
			{/if}
		</div>
	</div>

	{#if canShare}
		<ShareDialog bind:target={sharing} onsaved={() => app.sessionsVersion++} />
	{/if}

	<!-- the chosen laps become the reference, the selected ones follow -->
	<ReferencePicker
		bind:target={picking}
		exclude={selected}
		max={MAX_COMPARE - selected.length}
		onpick={(laps) => openCompare([...laps.map((l) => l.lapId), ...selectedByTime()])}
	/>

	{#if client.capabilities.exportFiles}
		<ExportDialog bind:open={exporting} scopes={exportScopes} />
	{/if}

	<!-- tires: compound, temperature and wear; all four on hover -->
	{#snippet tires(c: LapConditions)}
		{@const sum = tireSummary(c)}
		{@const wheels = wheelDetails(c)}
		<span class="group/tires relative inline-flex cursor-default items-center gap-3">
			{#if sum.compound}
				<span class="inline-flex items-center gap-1.5">
					<!-- 1 dot (all the same), 2 (front, rear) or 4 (FL FR RL RR); split
					     where changed in the lap: before | after -->
					<span class="inline-flex gap-1" aria-hidden="true">
						{#each compoundDots(c, game) as dot (dot.label)}
							<span
								class="inline-flex h-2.5 w-2.5 overflow-hidden rounded-full"
								title="{dot.label}: {dot.before ? `${dot.before.name} → ` : ''}{dot.after.name}"
							>
								{#if dot.before}<span class="h-full w-1/2 {dot.before.bg}"></span>{/if}
								<span class="h-full flex-1 {dot.after.bg}"></span>
							</span>
						{/each}
					</span>{sum.compound}
				</span>
			{/if}
			{#if sum.temp}<span class="tabular-nums">{sum.temp}</span>{/if}
			{#if sum.wear}<span class="tabular-nums">{sum.wear}</span>{/if}
			{#if anyFlat(c)}
				<span class="rounded bg-amber-950 px-1 text-xs text-amber-300">flat</span>
			{/if}
			<span class="sr-only">
				{#each wheels as w (w.key)}
					{w.label}: {[w.compound, w.temp, w.wear && `${w.wear} worn`, w.flat && 'flat']
						.filter(Boolean)
						.join(', ')}.
				{/each}
				{#if c.tyreCompound?.before}Tires changed in this lap.{/if}
			</span>
			<span
				class="pointer-events-none absolute top-full left-0 z-20 mt-1 hidden w-max grid-cols-2 gap-1.5 rounded border border-zinc-700 bg-zinc-900 p-2 text-left text-xs shadow-lg group-hover/tires:grid"
				aria-hidden="true"
			>
				{#each wheels as w (w.key)}
					<span class="min-w-36 rounded bg-zinc-800/60 px-2 py-1.5">
						<span class="mb-1 block text-zinc-400">{w.label}</span>
						{#if w.compound}
							<span class="flex items-center gap-1.5 text-zinc-100">
								<ConditionIcon
									name="tires"
									class="h-3.5 w-3.5 {compoundColor(w.compoundName, game).text}"
								/>{w.compound}
							</span>
						{/if}
						{#if w.temp}
							<span class="flex items-center gap-1.5 text-zinc-100 tabular-nums">
								<ConditionIcon name="tyreTemp" class="h-3.5 w-3.5 text-zinc-400" />{w.temp}
							</span>
						{/if}
						{#if w.wear}
							<span class="flex items-center gap-1.5 text-zinc-100 tabular-nums">
								<ConditionIcon name="tyreWear" class="h-3.5 w-3.5 text-zinc-400" />{w.wear}
								{#if w.flat}<span class="text-amber-300">flat</span>{/if}
							</span>
						{/if}
					</span>
				{/each}
				{#if c.tyreCompound?.before}
					<span class="col-span-2 px-1 text-zinc-400">Tires changed in this lap</span>
				{/if}
			</span>
		</span>
	{/snippet}

	{#if showChart}
		<LapTimeChart laps={visibleLaps} bestId={best?.id} />
	{/if}

	<table class="w-full text-left text-sm">
		<thead class="text-zinc-400">
			<tr>
				<th class="w-8 py-2"><span class="sr-only">Select</span></th>
				<th class="font-normal">Lap</th>
				<th class="text-right font-normal">Time</th>
				<th class="text-right font-normal">Δ best</th>
				{#each conditionColumns as col (col.key)}
					<th
						class="pl-4 font-normal"
						class:text-right={col.key !== 'weather' && col.key !== 'tires'}
						title={col.title}
					>
						<span class="inline-flex align-middle"><ConditionIcon name={col.key} /></span>
						<span class="sr-only">{col.title}</span>
					</th>
				{/each}
				<th class="pl-6 font-normal">Status</th>
				{#if canShare}<th class="font-normal"><span class="sr-only">Sharing</span></th>{/if}
			</tr>
		</thead>
		<tbody>
			{#each visibleLaps as l (l.id)}
				{@const isBest = best?.id === l.id}
				<tr
					class="border-t border-zinc-800 hover:bg-zinc-900"
					class:opacity-60={!l.valid || l.incomplete}
				>
					<td class="py-1.5">
						<input
							type="checkbox"
							aria-label="Select lap {l.number}"
							checked={selected.includes(l.id)}
							disabled={!selected.includes(l.id) && selected.length >= MAX_COMPARE}
							onchange={() => toggle(l.id)}
						/>
					</td>
					<td>{l.number}</td>
					<td class="text-right font-mono" class:text-purple-300={isBest}>
						{formatLapTime(l.lapTimeMs)}
					</td>
					<td class="text-right font-mono text-zinc-400">
						<!-- an out-lap is often partial (F1 time trial: garage to line); no delta -->
						{#if best && l.lapTimeMs !== null && !isBest && !l.outLap}
							{formatDelta(l.lapTimeMs - best.lapTimeMs!)}
						{/if}
					</td>
					{#each conditionColumns as col (col.key)}
						{@const c = l.conditions}
						{#if col.key === 'tires'}
							<td class="pl-4 whitespace-nowrap">
								{#if c && cellText(c, col.key)}
									{@render tires(c)}
								{/if}
							</td>
						{:else if col.key === 'weather'}
							<td class="max-w-48 pl-4 text-xs text-zinc-300">{cellText(c, col.key)}</td>
						{:else}
							<td
								class="pl-4 text-right whitespace-nowrap tabular-nums"
								title={col.key === 'fuel' ? fuelUsedText(c) : undefined}
							>
								{cellText(c, col.key)}
							</td>
						{/if}
					{/each}
					<td class="pl-6">
						{#if isBest}<span class="mr-1 rounded bg-purple-900 px-1.5 text-xs text-purple-200"
								>best</span
							>{/if}
						{#each status(l) as tag (tag)}
							<span class="mr-1 rounded bg-zinc-800 px-1.5 text-xs text-zinc-300">{tag}</span>
						{/each}
						{#if !l.valid && l.invalidReason}
							<span class="text-xs text-amber-400/80">{l.invalidReason}</span>
						{/if}
					</td>
					{#if canShare}
						<td class="text-right">
							<button
								type="button"
								class="rounded border border-zinc-800 px-2 py-0.5 text-xs text-zinc-400 hover:bg-zinc-800 hover:text-zinc-200"
								aria-label="Share lap {l.number}"
								onclick={() => (sharing = { kind: 'lap', id: l.id, label: `Lap ${l.number}` })}
								>Share…</button
							>
						</td>
					{/if}
				</tr>
			{/each}
		</tbody>
	</table>
	{#if hiddenCount > 0 || showAll}
		<button
			type="button"
			class="mt-3 text-sm text-zinc-400 hover:text-zinc-200"
			onclick={() => (showAll = !showAll)}
		>
			{showAll ? 'Hide incomplete laps' : `Show ${hiddenCount} incomplete lap(s)`}
		</button>
	{/if}
{/if}
