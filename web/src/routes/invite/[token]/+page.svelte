<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { api, type InviteLinkPreview } from '#lib/api/index.ts';
	import { errorText } from '#lib/app-state.svelte.ts';

	const server = api().server;
	const token = $derived(page.params.token ?? '');

	let preview = $state<InviteLinkPreview | null>(null);
	let error = $state<string | null>(null);
	let busy = $state(false);

	$effect(() => {
		const t = token;
		preview = null;
		error = null;
		server
			.inviteLink(t)
			.then((p) => (preview = p))
			.catch((e) => (error = errorText(e)));
	});

	async function join() {
		busy = true;
		try {
			const team = await server.acceptInviteLink(token);
			await goto(resolve('/teams/[id]', { id: team.id }));
		} catch (e) {
			error = errorText(e);
		} finally {
			busy = false;
		}
	}
</script>

<h1 class="mb-4 text-2xl font-semibold">Team invitation</h1>

{#if error}
	<p class="text-red-400" role="alert">{error}</p>
	<p class="mt-2 text-sm text-zinc-400">
		Ask for a new link, or see <a class="underline" href={resolve('/teams')}>your teams</a>.
	</p>
{:else if !preview}
	<p class="text-zinc-400">Loading…</p>
{:else}
	<div class="max-w-xl rounded border border-zinc-800 p-4">
		<p>
			{#if preview.invitedBy}
				{preview.invitedBy.displayName} (@{preview.invitedBy.username}) invites you to the team
			{:else}
				You are invited to the team
			{/if}
			<strong>{preview.team.name}</strong>.
		</p>
		<p class="mt-2 text-sm text-zinc-400">
			As a member you see the sessions and laps the others share with the team. You decide yourself
			what you share.
		</p>
		<div class="mt-4 flex gap-2">
			{#if preview.alreadyMember}
				<a
					class="rounded bg-zinc-100 px-3 py-1.5 text-sm font-medium text-zinc-900 hover:bg-white"
					href={resolve('/teams/[id]', { id: preview.team.id })}
				>
					You are a member – open team
				</a>
			{:else}
				<button
					type="button"
					class="rounded bg-zinc-100 px-3 py-1.5 text-sm font-medium text-zinc-900 hover:bg-white disabled:opacity-50"
					disabled={busy}
					onclick={join}
				>
					Join {preview.team.name}
				</button>
				<a
					class="rounded border border-zinc-700 px-3 py-1.5 text-sm text-zinc-300 hover:bg-zinc-800"
					href={resolve('/')}
				>
					Not now
				</a>
			{/if}
		</div>
	</div>
{/if}
