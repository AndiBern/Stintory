<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { api, type MyInvite, type TeamSummary } from '#lib/api/index.ts';
	import { errorText, sharingAvailable } from '#lib/app-state.svelte.ts';

	const server = api().server;

	let teams = $state<TeamSummary[] | null>(null);
	let invites = $state<MyInvite[]>([]);
	let error = $state<string | null>(null);
	let name = $state('');

	async function load() {
		try {
			[teams, invites] = await Promise.all([server.teams(), server.myInvites()]);
			error = null;
		} catch (e) {
			error = errorText(e);
		}
	}

	$effect(() => {
		if (sharingAvailable()) void load();
	});

	async function create(event: SubmitEvent) {
		event.preventDefault();
		try {
			const team = await server.createTeam(name);
			name = '';
			await goto(resolve('/teams/[id]', { id: team.id }));
		} catch (e) {
			error = errorText(e);
		}
	}

	async function answer(invite: MyInvite, accept: boolean) {
		try {
			if (accept) await server.acceptInvite(invite.id);
			else await server.declineInvite(invite.id);
			await load();
		} catch (e) {
			error = errorText(e);
		}
	}

	const input =
		'rounded border border-zinc-700 bg-zinc-900 px-3 py-1.5 text-sm focus:border-zinc-400 focus:outline-none';
	const button =
		'rounded bg-zinc-100 px-3 py-1.5 text-sm font-medium text-zinc-900 hover:bg-white disabled:opacity-50';
	const small = 'rounded border border-zinc-700 px-2 py-0.5 text-xs hover:bg-zinc-800';
</script>

<h1 class="mb-4 text-2xl font-semibold">Teams</h1>

{#if !sharingAvailable()}
	<p class="max-w-xl text-zinc-400">
		Teams live on a Stintory server. Connect this app to a server under
		<a class="underline" href={resolve('/settings')}>Settings</a> to create or join teams.
	</p>
{:else}
	{#if error}
		<p class="mb-4 text-sm text-red-400" role="alert">{error}</p>
	{/if}

	{#if invites.length > 0}
		<section class="mb-6 max-w-3xl rounded border border-sky-900 bg-sky-950/40 p-4">
			<h2 class="mb-2 font-medium">Invitations</h2>
			<ul class="flex flex-col gap-2 text-sm">
				{#each invites as invite (invite.id)}
					<li class="flex flex-wrap items-center justify-between gap-2">
						<span>
							<strong>{invite.team.name}</strong>
							{#if invite.invitedBy}
								<span class="text-zinc-400">· invited by {invite.invitedBy.displayName}</span>
							{/if}
						</span>
						<span class="flex gap-2">
							<button type="button" class={button} onclick={() => answer(invite, true)}>Join</button
							>
							<button type="button" class={small} onclick={() => answer(invite, false)}
								>Decline</button
							>
						</span>
					</li>
				{/each}
			</ul>
		</section>
	{/if}

	<section class="mb-6 max-w-3xl rounded border border-zinc-800 p-4">
		<h2 class="mb-3 font-medium">My teams</h2>
		{#if teams === null}
			{#if !error}<p class="text-sm text-zinc-400">Loading…</p>{/if}
		{:else if teams.length === 0}
			<p class="text-sm text-zinc-400">
				You are not in a team yet. Create one and invite your team mates, or ask them for an invite
				link.
			</p>
		{:else}
			<table class="w-full text-left text-sm">
				<thead class="text-zinc-400">
					<tr>
						<th class="py-1 font-normal">Team</th>
						<th class="font-normal">Your role</th>
						<th class="text-right font-normal">Members</th>
					</tr>
				</thead>
				<tbody>
					{#each teams as team (team.id)}
						<tr class="border-t border-zinc-800">
							<td class="py-1.5">
								<a class="hover:underline" href={resolve('/teams/[id]', { id: team.id })}
									>{team.name}</a
								>
							</td>
							<td class="text-zinc-300">{team.role}</td>
							<td class="text-right">{team.memberCount}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}

		<form class="mt-4 flex flex-wrap items-end gap-2" onsubmit={create}>
			<label class="flex flex-col gap-1 text-sm">
				<span class="text-zinc-400">New team</span>
				<input class={input} bind:value={name} required maxlength="64" placeholder="Name" />
			</label>
			<button type="submit" class={button} disabled={!name.trim()}>Create team</button>
		</form>
	</section>

	<p class="max-w-3xl text-sm text-zinc-500">
		Share sessions and laps with a team from a session's "Share…" button, or share everything you
		record by default under <a class="underline" href={resolve('/shared')}>Sharing</a>.
	</p>
{/if}
