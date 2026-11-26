<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts" module>
	/** What the dialog shares. */
	export interface ShareTarget {
		kind: 'session' | 'lap';
		id: string;
		/** e.g. "Lap 7" or the track name */
		label: string;
	}
</script>

<script lang="ts">
	import { api, type Grantee, type SharingMode, type SharingState } from '#lib/api/index.ts';
	import { app, errorText } from '#lib/app-state.svelte.ts';
	import { formatAudience } from '#lib/format.ts';
	import GranteePicker from './GranteePicker.svelte';

	let {
		target = $bindable(null),
		onsaved
	}: {
		/** Set to open the dialog; null when closed */
		target: ShareTarget | null;
		onsaved?: () => void;
	} = $props();

	const client = api();
	const server = client.server;
	let dialog: HTMLDialogElement | undefined = $state();
	let sharing = $state<SharingState | null>(null);
	let mode = $state<SharingMode>('inherit');
	let grants = $state<Grantee[]>([]);
	let error = $state<string | null>(null);
	let busy = $state(false);

	$effect(() => {
		if (!dialog) return;
		const t = target;
		if (t && !dialog.open) {
			sharing = null;
			error = null;
			busy = false;
			dialog.showModal();
			(t.kind === 'session' ? server.sessionSharing(t.id) : server.lapSharing(t.id))
				.then((s) => {
					sharing = s;
					mode = s.mode;
					// start a custom audience from what applies now
					grants = s.mode === 'custom' ? s.grants : s.effective;
				})
				.catch((e) => {
					error = errorText(e);
					if (client.capabilities.serverSync) {
						error += ' (Sharing works once the session is synced to the server.)';
					}
				});
		} else if (!t && dialog.open) {
			dialog.close();
		}
	});

	const inheritLabel = $derived(
		target?.kind === 'lap' ? "Same as the lap's session" : 'Use my default'
	);
	const inherited = $derived.by(() => {
		if (!sharing) return '';
		if (sharing.source === 'imported') {
			return 'Only you: imported sessions are not shared by default.';
		}
		// with "custom" the server reports the object's own audience
		return sharing.mode === 'inherit' ? formatAudience(sharing.effective) : '';
	});

	async function save(event: SubmitEvent) {
		event.preventDefault();
		if (!target || !sharing || busy) return;
		busy = true;
		error = null;
		try {
			const update = {
				mode,
				grants: sharing.everyoneAllowed ? grants : grants.filter((g) => g.kind !== 'everyone')
			};
			const saved =
				target.kind === 'session'
					? await server.setSessionSharing(target.id, update)
					: await server.setLapSharing(target.id, update);
			app.notice = {
				tone: 'ok',
				text: `${target.label} is now visible to: ${formatAudience(saved.effective)}`
			};
			target = null;
			onsaved?.();
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
	class="m-auto w-[30rem] max-w-[calc(100vw-2rem)] rounded border border-zinc-700 bg-zinc-900 p-0 text-zinc-100 backdrop:bg-black/60"
	aria-labelledby="share-title"
>
	<form class="flex flex-col gap-4 p-5" onsubmit={save}>
		<h2 id="share-title" class="text-lg font-semibold">Share {target?.label ?? ''}</h2>

		{#if error}<p class="text-sm text-red-400" role="alert">{error}</p>{/if}

		{#if sharing}
			<fieldset class="flex flex-col gap-2 text-sm">
				<legend class="sr-only">Who can see it</legend>
				<label class="flex items-start gap-2">
					<input type="radio" name="mode" value="inherit" bind:group={mode} class="mt-1" />
					<span>
						{inheritLabel}
						{#if inherited}
							<span class="block text-xs text-zinc-400">Now: {inherited}</span>
						{/if}
					</span>
				</label>
				<label class="flex items-start gap-2">
					<input type="radio" name="mode" value="custom" bind:group={mode} class="mt-1" />
					<span>
						Choose who can see {target?.kind === 'lap' ? 'this lap' : 'this session'}
						<span class="block text-xs text-zinc-400">
							{target?.kind === 'lap'
								? 'Independent of the session, also when the session is shared differently.'
								: 'Independent of your default. Laps with their own choice keep it.'}
						</span>
					</span>
				</label>
			</fieldset>

			{#if mode === 'custom'}
				<div class="rounded border border-zinc-800 p-3">
					<GranteePicker bind:grants everyoneAllowed={sharing.everyoneAllowed} />
				</div>
				<p class="text-sm">
					<span class="text-zinc-400">Visible to:</span>
					{formatAudience(
						sharing.everyoneAllowed ? grants : grants.filter((g) => g.kind !== 'everyone')
					)}
				</p>
			{/if}
			<p class="text-xs text-zinc-500">
				Others can look at the laps and compare them with theirs, but not change or delete them.
				They see the car setups only where you chose “Setups too”. Copies someone exported can't be
				taken back.
			</p>
		{:else if !error}
			<p class="text-sm text-zinc-400">Loading…</p>
		{/if}

		<div class="flex justify-end gap-2">
			<button
				type="button"
				class="rounded border border-zinc-700 px-3 py-1.5 text-sm hover:bg-zinc-800"
				onclick={() => (target = null)}>Cancel</button
			>
			<button
				type="submit"
				class="rounded bg-zinc-100 px-3 py-1.5 text-sm font-medium text-zinc-900 hover:bg-white disabled:opacity-50"
				disabled={busy || !sharing}
			>
				{busy ? 'Saving…' : 'Save'}
			</button>
		</div>
	</form>
</dialog>
