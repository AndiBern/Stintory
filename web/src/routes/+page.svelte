<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script module lang="ts">
	import {
		DEFAULT_PAGE_SIZE,
		DEFAULT_SORT,
		NO_FILTER,
		loadFilter,
		type SessionFilter,
		type Sort,
		type SortKey
	} from '#lib/session-list.ts';

	/**
	 * Sort order and page survive opening a session and coming back, the
	 * filters also the next start.
	 */
	const view = $state<{ filter: SessionFilter; sort: Sort; page: number; size: number }>({
		filter: loadFilter(),
		sort: { ...DEFAULT_SORT },
		page: 1,
		size: DEFAULT_PAGE_SIZE
	});
</script>

<script lang="ts">
	import { confirmAction } from '#lib/confirm.ts';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { api, type SessionSummary } from '#lib/api/index.ts';
	import { app, errorText, runImport } from '#lib/app-state.svelte.ts';
	import Combobox from '#lib/components/Combobox.svelte';
	import ExportDialog, { type ExportScope } from '#lib/components/ExportDialog.svelte';
	import {
		GAMES,
		SESSION_TYPES,
		fileDate,
		fileName,
		formatDateTime,
		formatGame,
		formatLapTime,
		formatOrigin,
		formatSessionType
	} from '#lib/format.ts';
	import {
		filterOptions,
		filterSessions,
		isFiltered,
		isSharedCopy,
		nextSort,
		PAGE_SIZES,
		pageForRow,
		pageNumbers,
		paginate,
		saveFilter,
		sortSessions,
		withoutUnknownNames
	} from '#lib/session-list.ts';

	const client = api();
	let sessions: Promise<SessionSummary[]> = $state(new Promise(() => {}));
	let list: SessionSummary[] = $state([]);
	let selected = $state<string[]>([]);
	let exporting = $state(false);
	let deleting = $state(false);
	let table = $state<HTMLTableElement>();

	$effect(() => {
		void app.sessionsVersion; // refetch when sessions changed
		const pending = client.listSessions();
		sessions = pending;
		pending
			.then((l) => {
				list = l;
				selected = selected.filter((id) => l.some((s) => s.id === id));
				view.filter = withoutUnknownNames(view.filter, l);
				// without the owner filter on screen, a remembered "shared" could not be undone
				if (!showOwner && view.filter.owner !== NO_FILTER.owner) {
					view.filter.owner = NO_FILTER.owner;
				}
			})
			.catch(() => {}); // shown by the {#await} block
	});

	$effect(() => saveFilter(view.filter));

	const visible = $derived(sortSessions(filterSessions(list, view.filter), view.sort));
	const paged = $derived(paginate(visible, view.page, view.size));
	const options = $derived(filterOptions(list, view.filter));
	/**
	 * Only the desktop app keeps copies of sessions others shared; the server's
	 * list has the user's own sessions only (shared ones are under "Sharing").
	 */
	const showOwner = $derived(client.capabilities.serverSync || list.some(isSharedCopy));
	/**
	 * Selected rows hidden by a filter are left alone by export and delete;
	 * those on other pages are not.
	 */
	const chosen = $derived(visible.filter((s) => selected.includes(s.id)));
	const pageChosen = $derived(paged.items.filter((s) => selected.includes(s.id)).length);
	const pageAllChosen = $derived(paged.items.length > 0 && pageChosen === paged.items.length);

	/** A shared session on the server isn't the viewer's to delete; the desktop app removes its copy. */
	const deletable = $derived(
		chosen.filter((s) => s.origin.kind !== 'synced' || client.capabilities.localSettings)
	);

	function open(id: string) {
		goto(resolve('/session/[id]', { id }));
	}

	function toggle(id: string) {
		selected = selected.includes(id) ? selected.filter((s) => s !== id) : [...selected, id];
	}

	function togglePage() {
		const ids = paged.items.map((s) => s.id);
		selected = pageAllChosen
			? selected.filter((id) => !ids.includes(id))
			: [...new Set([...selected, ...ids])];
	}

	function selectAllVisible() {
		selected = [...new Set([...selected, ...visible.map((s) => s.id)])];
	}

	function clearVisibleSelection() {
		const ids = new Set(visible.map((s) => s.id));
		selected = selected.filter((id) => !ids.has(id));
	}

	function goToPage(page: number) {
		view.page = page;
		// the page buttons are below the table: show the new page from its top
		if (table && table.getBoundingClientRect().top < 0) table.scrollIntoView({ block: 'start' });
	}

	function setPageSize(size: number) {
		view.page = pageForRow(paged.start, size);
		view.size = size;
	}

	/** Keeps a chosen value selectable even when the other filters no longer offer it. */
	function withCurrent(names: string[], current: string): string[] {
		return current && !names.includes(current) ? [current, ...names] : names;
	}

	async function removeSelected() {
		const targets = deletable;
		if (targets.length === 0) return;
		const laps = targets.reduce((n, s) => n + s.lapCount, 0);
		const copies = targets.filter((s) => s.origin.kind === 'synced').length;
		const what =
			targets.length === 1
				? `this session (${targets[0].trackName}, ${laps} laps)`
				: `${targets.length} sessions with ${laps} laps`;
		const note =
			copies > 0 ? ` Copies of sessions shared with you are only removed from this computer.` : '';
		if (!(await confirmAction(`Delete ${what}? This cannot be undone.${note}`))) return;

		deleting = true;
		let failed = 0;
		let lastError = '';
		for (const s of targets) {
			try {
				await client.deleteSession(s.id);
			} catch (e) {
				failed++;
				lastError = errorText(e);
			}
		}
		deleting = false;
		selected = [];
		app.sessionsVersion++;
		if (failed > 0) {
			app.notice = {
				tone: 'error',
				text: `Could not delete ${failed} of ${targets.length} sessions: ${lastError}`
			};
		}
	}

	const exportScopes: ExportScope[] = $derived.by(() => {
		const first = chosen[0];
		if (!first) return [];
		const laps = chosen.reduce((n, s) => n + s.lapCount, 0);
		return [
			{
				label:
					chosen.length === 1
						? `${first.trackName}, ${first.carName} (${laps} laps)`
						: `${chosen.length} sessions (${laps} laps)`,
				selection: { sessionIds: chosen.map((s) => s.id) },
				fileName:
					chosen.length === 1
						? fileName(first.trackName, first.carName, fileDate(first.startedAt))
						: fileName('Stintory', `${chosen.length} sessions`, fileDate(new Date().toISOString()))
			}
		];
	});

	const control =
		'h-8 rounded border border-zinc-700 bg-zinc-900 px-2 text-sm text-zinc-100 focus:border-zinc-400 focus:outline-none';
	const pageButton =
		'h-8 min-w-8 rounded px-2 tabular-nums enabled:hover:bg-zinc-800 enabled:hover:text-zinc-100 disabled:opacity-40';
	/** Fixed widths keep the columns in place while filtering. */
	const columns: { key: SortKey; label: string; width: string; right?: boolean }[] = [
		{ key: 'date', label: 'Date', width: 'w-48' },
		{ key: 'track', label: 'Track', width: '' },
		{ key: 'car', label: 'Car', width: 'w-1/4' },
		{ key: 'type', label: 'Type', width: 'w-28' },
		{ key: 'laps', label: 'Laps', width: 'w-16', right: true },
		{ key: 'best', label: 'Best', width: 'w-28', right: true }
	];
</script>

<div class="mb-4 flex items-center justify-between gap-2">
	<h1 class="text-2xl font-semibold">Sessions</h1>
	<div class="flex gap-2">
		<button
			type="button"
			class="rounded border border-zinc-700 px-3 py-1.5 text-sm text-zinc-300 enabled:hover:border-red-700 enabled:hover:text-red-300 disabled:opacity-40"
			disabled={deletable.length === 0 || deleting}
			onclick={removeSelected}
		>
			{deleting ? 'Deleting…' : deletable.length > 0 ? `Delete ${deletable.length}…` : 'Delete…'}
		</button>
		{#if client.capabilities.exportFiles}
			<button
				type="button"
				class="rounded border border-zinc-700 px-3 py-1.5 text-sm hover:bg-zinc-800 disabled:opacity-40"
				disabled={chosen.length === 0}
				onclick={() => (exporting = true)}
			>
				{chosen.length > 0 ? `Export ${chosen.length}…` : 'Export…'}
			</button>
		{/if}
		{#if client.capabilities.importFiles}
			<button
				type="button"
				class="rounded bg-zinc-100 px-3 py-1.5 text-sm font-medium text-zinc-900 hover:bg-white disabled:opacity-50"
				disabled={app.importing !== null}
				onclick={() => runImport(() => client.pickAndImportFiles())}
			>
				Import…
			</button>
		{/if}
	</div>
</div>

{#if client.capabilities.exportFiles}
	<ExportDialog bind:open={exporting} scopes={exportScopes} />
{/if}

{#await sessions}
	<p class="text-zinc-400">Loading…</p>
{:then}
	{#if list.length === 0}
		<div class="rounded border border-dashed border-zinc-700 p-8 text-center text-zinc-400">
			<p>No sessions yet.</p>
			<p class="mt-1 text-sm">
				{#if client.capabilities.liveRecording}
					Start driving – sessions are recorded automatically. Or import Stintory <code
						>.stintory</code
					>
					or iRacing <code>.ibt</code> files (drag them onto this window).
				{:else}
					Sync from the desktop app, or upload <code>.stintory</code> files exported there with "Import…".
				{/if}
			</p>
		</div>
	{:else}
		<form
			class="mb-4 flex flex-wrap items-end gap-3 text-xs text-zinc-400"
			aria-label="Filter sessions"
			onsubmit={(e) => e.preventDefault()}
			onchange={() => (view.page = 1)}
		>
			{#if showOwner}
				<label class="flex flex-col gap-1">
					Owner
					<select class={control} bind:value={view.filter.owner}>
						<option value="mine">Me</option>
						<option value="shared">Shared with me</option>
						<option value="all">All</option>
					</select>
				</label>
			{/if}
			<label class="flex flex-col gap-1">
				Game
				<select class={control} bind:value={view.filter.game}>
					<option value="">All games</option>
					{#each Object.entries(GAMES) as [id, name] (id)}
						<option value={id}>{name}</option>
					{/each}
				</select>
			</label>
			<div class="w-56">
				<Combobox
					label="Track"
					placeholder="All tracks"
					class={control}
					options={withCurrent(options.tracks, view.filter.track)}
					bind:value={view.filter.track}
					onchange={() => (view.page = 1)}
				/>
			</div>
			<div class="w-56">
				<Combobox
					label="Car"
					placeholder="All cars"
					class={control}
					options={withCurrent(options.cars, view.filter.car)}
					bind:value={view.filter.car}
					onchange={() => (view.page = 1)}
				/>
			</div>
			<label class="flex flex-col gap-1">
				Type
				<select class={control} bind:value={view.filter.type}>
					<option value="">All types</option>
					{#each Object.entries(SESSION_TYPES) as [id, name] (id)}
						<option value={id}>{name}</option>
					{/each}
				</select>
			</label>
			<!-- one group, so the range never wraps apart -->
			<fieldset class="flex flex-col gap-1">
				<legend class="contents">Date</legend>
				<div class="flex items-center gap-1.5">
					<input
						type="date"
						class={control}
						aria-label="From"
						bind:value={view.filter.from}
						max={view.filter.to || undefined}
					/>
					<span aria-hidden="true">–</span>
					<input
						type="date"
						class={control}
						aria-label="To"
						bind:value={view.filter.to}
						min={view.filter.from || undefined}
					/>
				</div>
			</fieldset>
			{#if isFiltered(view.filter)}
				<button
					type="button"
					class="py-1.5 text-sm text-zinc-400 hover:text-zinc-100"
					onclick={() => {
						view.filter = { ...NO_FILTER };
						view.page = 1;
					}}
				>
					Clear filters
				</button>
				<span class="py-1.5 text-sm" role="status">{visible.length} of {list.length} sessions</span>
			{/if}
		</form>

		<table class="w-full table-fixed text-left text-sm" bind:this={table}>
			<thead class="text-zinc-400">
				<tr>
					<th class="w-8 py-2">
						<input
							type="checkbox"
							aria-label="Select all sessions on this page"
							checked={pageAllChosen}
							indeterminate={pageChosen > 0 && !pageAllChosen}
							disabled={paged.items.length === 0}
							onchange={togglePage}
						/>
					</th>
					{#each columns as col (col.key)}
						{@const active = view.sort.key === col.key}
						<th
							class="font-normal {col.width}"
							class:text-right={col.right}
							aria-sort={active ? (view.sort.descending ? 'descending' : 'ascending') : 'none'}
						>
							<button
								type="button"
								class="inline-flex items-center gap-1 py-2 hover:text-zinc-100"
								class:flex-row-reverse={col.right}
								class:text-zinc-200={active}
								onclick={() => {
									view.sort = nextSort(view.sort, col.key);
									view.page = 1;
								}}
							>
								{col.label}
								<span class="inline-block w-3 text-xs" aria-hidden="true"
									>{active ? (view.sort.descending ? '↓' : '↑') : ''}</span
								>
							</button>
						</th>
					{/each}
				</tr>
				{#if pageAllChosen && paged.pageCount > 1}
					<tr>
						<td colspan="7" class="bg-zinc-900 py-1.5 text-center text-xs text-zinc-300">
							{#if chosen.length < visible.length}
								All {paged.items.length} sessions on this page are selected.
								<button
									type="button"
									class="ml-1 text-sky-300 hover:text-sky-200"
									onclick={selectAllVisible}
								>
									Select all {visible.length}
								</button>
							{:else}
								All {visible.length} sessions are selected.
								<button
									type="button"
									class="ml-1 text-sky-300 hover:text-sky-200"
									onclick={clearVisibleSelection}
								>
									Clear selection
								</button>
							{/if}
						</td>
					</tr>
				{/if}
			</thead>
			<tbody>
				{#each paged.items as s (s.id)}
					<tr
						class="cursor-pointer border-t border-zinc-800 hover:bg-zinc-900"
						onclick={() => open(s.id)}
						onkeydown={(e) => e.key === 'Enter' && open(s.id)}
						tabindex="0"
					>
						<td class="py-2">
							<input
								type="checkbox"
								aria-label="Select session {s.trackName} {formatDateTime(s.startedAt)}"
								checked={selected.includes(s.id)}
								onclick={(e) => e.stopPropagation()}
								onkeydown={(e) => e.stopPropagation()}
								onchange={() => toggle(s.id)}
							/>
						</td>
						<td class="py-2 whitespace-nowrap">{formatDateTime(s.startedAt)}</td>
						<td>
							<div>{s.trackName}</div>
							<div class="text-xs text-zinc-500">
								{formatGame(s.game)}
								{#if formatOrigin(s.origin)}
									<span class="ml-1 rounded bg-sky-950 px-1.5 text-sky-300"
										>{formatOrigin(s.origin)}</span
									>
								{/if}
								{#if s.shared}
									<span class="ml-1 rounded bg-emerald-950 px-1.5 text-emerald-300">shared</span>
								{/if}
							</div>
						</td>
						<td>{s.carName}</td>
						<td>{formatSessionType(s.sessionType)}</td>
						<td class="text-right">{s.lapCount}</td>
						<td class="text-right font-mono">{formatLapTime(s.bestLapMs)}</td>
					</tr>
				{:else}
					<tr class="border-t border-zinc-800">
						<td colspan="7" class="py-6 text-center text-zinc-400">
							{isFiltered(view.filter)
								? 'No sessions match the filters.'
								: 'No sessions of your own yet.'}
						</td>
					</tr>
				{/each}
			</tbody>
		</table>

		{#if visible.length > PAGE_SIZES[0]}
			<nav
				class="mt-3 flex flex-wrap items-center justify-between gap-3 border-t border-zinc-800 pt-3 text-sm text-zinc-400"
				aria-label="Pages"
			>
				<span role="status">
					{paged.start + 1}–{paged.start + paged.items.length} of {visible.length}
				</span>
				<div class="flex items-center gap-1">
					<button
						type="button"
						class={pageButton}
						aria-label="Previous page"
						disabled={paged.page === 1}
						onclick={() => goToPage(paged.page - 1)}
					>
						‹
					</button>
					{#each pageNumbers(paged.page, paged.pageCount) as p, i (i)}
						{#if p === null}
							<span class="px-1" aria-hidden="true">…</span>
						{:else}
							<button
								type="button"
								class="{pageButton} {p === paged.page ? 'bg-zinc-800 text-zinc-100' : ''}"
								aria-label="Page {p}"
								aria-current={p === paged.page ? 'page' : undefined}
								onclick={() => goToPage(p)}
							>
								{p}
							</button>
						{/if}
					{/each}
					<button
						type="button"
						class={pageButton}
						aria-label="Next page"
						disabled={paged.page === paged.pageCount}
						onclick={() => goToPage(paged.page + 1)}
					>
						›
					</button>
				</div>
				<label class="flex items-center gap-2">
					Per page
					<select
						class={control}
						value={view.size}
						onchange={(e) => setPageSize(Number(e.currentTarget.value))}
					>
						{#each PAGE_SIZES as size (size)}
							<option value={size}>{size}</option>
						{/each}
					</select>
				</label>
			</nav>
		{/if}
	{/if}
{:catch error}
	<p class="text-red-400" role="alert">Could not load sessions: {error.message}</p>
{/await}
