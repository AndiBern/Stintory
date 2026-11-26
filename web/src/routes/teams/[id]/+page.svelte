<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { confirmAction } from '#lib/confirm.ts';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import {
		api,
		type TeamDetail,
		type TeamInvite,
		type TeamMember,
		type TeamRole
	} from '#lib/api/index.ts';
	import { app, auth, errorText } from '#lib/app-state.svelte.ts';
	import { formatDateTime } from '#lib/format.ts';

	const server = api().server;
	const id = $derived(page.params.id ?? '');

	let team = $state<TeamDetail | null>(null);
	let error = $state<string | null>(null);
	let inviteName = $state('');
	let inviteMsg = $state<string | null>(null);
	let linkDays = $state(7);
	let linkUses = $state<number | null>(null);
	/** A new invite link, shown once */
	let newLink = $state<string | null>(null);
	let copied = $state(false);
	let newName = $state('');
	let renaming = $state(false);

	async function load() {
		try {
			team = await server.team(id);
			error = null;
		} catch (e) {
			error = errorText(e);
		}
	}

	$effect(() => {
		void id;
		team = null;
		newLink = null;
		void load();
	});

	const isOwner = $derived(team?.role === 'owner');
	const canManage = $derived(team !== null && team.role !== 'member');
	/** The logged-in user: known in the browser, by username in the desktop app. */
	function isMe(m: TeamMember): boolean {
		return auth.me ? m.user.id === auth.me.id : m.user.username === app.sync?.username;
	}

	function canChange(m: TeamMember): boolean {
		return isOwner || (canManage && m.role !== 'owner');
	}

	const roles = $derived<TeamRole[]>(isOwner ? ['member', 'admin', 'owner'] : ['member', 'admin']);

	async function run(action: () => Promise<unknown>, question?: string) {
		if (question && !(await confirmAction(question))) return;
		try {
			await action();
			await load();
		} catch (e) {
			error = errorText(e);
			await load().catch(() => {});
		}
	}

	async function invite(event: SubmitEvent) {
		event.preventDefault();
		inviteMsg = null;
		try {
			const inv = await server.inviteUser(id, inviteName.trim().replace(/^@/, ''));
			inviteMsg = `Invited ${inv.invitee?.displayName ?? inviteName}. They see the invitation under "Teams".`;
			inviteName = '';
			await load();
		} catch (e) {
			inviteMsg = errorText(e);
		}
	}

	async function createLink(event: SubmitEvent) {
		event.preventDefault();
		try {
			const inv = await server.createInviteLink(id, linkDays, linkUses || null);
			newLink = inv.url ?? null;
			copied = false;
			await load();
		} catch (e) {
			error = errorText(e);
		}
	}

	async function copyLink() {
		if (!newLink) return;
		try {
			await navigator.clipboard.writeText(newLink);
			copied = true;
		} catch {
			// select and copy by hand
		}
	}

	async function rename(event: SubmitEvent) {
		event.preventDefault();
		await run(() => server.renameTeam(id, newName));
		renaming = false;
	}

	async function leave() {
		if (
			!team ||
			!(await confirmAction(`Leave ${team.name}? You lose access to what is shared with the team.`))
		)
			return;
		try {
			await server.leaveTeam(id);
			await goto(resolve('/teams'));
		} catch (e) {
			error = errorText(e);
		}
	}

	async function remove() {
		if (
			!team ||
			!(await confirmAction(
				`Delete ${team.name}? Everything shared with the team stops being shared.`
			))
		)
			return;
		try {
			await server.deleteTeam(id);
			await goto(resolve('/teams'));
		} catch (e) {
			error = errorText(e);
		}
	}

	function inviteText(i: TeamInvite): string {
		if (i.kind === 'user') return i.invitee ? `@${i.invitee.username}` : 'A user';
		const uses = i.maxUses === null ? `used ${i.uses}×` : `used ${i.uses} of ${i.maxUses}×`;
		return `Invite link · ${uses} · until ${formatDateTime(i.expiresAt)}`;
	}

	const input =
		'rounded border border-zinc-700 bg-zinc-900 px-3 py-1.5 text-sm focus:border-zinc-400 focus:outline-none';
	const button =
		'rounded bg-zinc-100 px-3 py-1.5 text-sm font-medium text-zinc-900 hover:bg-white disabled:opacity-50';
	const small = 'rounded border border-zinc-700 px-2 py-0.5 text-xs hover:bg-zinc-800';
	const plain =
		'rounded border border-zinc-700 px-3 py-1.5 text-sm text-zinc-300 hover:bg-zinc-800';
</script>

<a href={resolve('/teams')} class="text-sm text-zinc-400 hover:text-zinc-200">← Teams</a>

{#if error}
	<p class="mt-2 mb-4 text-sm text-red-400" role="alert">{error}</p>
{/if}

{#if !team}
	{#if !error}<p class="mt-2 text-zinc-400">Loading…</p>{/if}
{:else}
	<div class="mt-2 mb-4 flex flex-wrap items-start justify-between gap-4">
		<div>
			{#if renaming}
				<form class="flex gap-2" onsubmit={rename}>
					<input
						class={input}
						bind:value={newName}
						required
						maxlength="64"
						aria-label="Team name"
					/>
					<button type="submit" class={button}>Save</button>
					<button type="button" class={plain} onclick={() => (renaming = false)}>Cancel</button>
				</form>
			{:else}
				<h1 class="text-2xl font-semibold">{team.name}</h1>
			{/if}
			<p class="mt-1 text-sm text-zinc-400">
				{team.memberCount}
				{team.memberCount === 1 ? 'member' : 'members'} · you are {team.role === 'member'
					? 'a member'
					: team.role === 'admin'
						? 'an admin'
						: 'an owner'}
			</p>
		</div>
		<div class="flex gap-2">
			{#if isOwner && !renaming}
				<button
					type="button"
					class={plain}
					onclick={() => {
						newName = team!.name;
						renaming = true;
					}}
				>
					Rename
				</button>
			{/if}
			<button type="button" class={plain} onclick={leave}>Leave team</button>
			{#if isOwner}
				<button
					type="button"
					class="{plain} hover:border-red-700 hover:text-red-300"
					onclick={remove}
				>
					Delete team
				</button>
			{/if}
		</div>
	</div>

	<section class="mb-6 max-w-4xl rounded border border-zinc-800 p-4">
		<h2 class="mb-3 font-medium">Members</h2>
		<table class="w-full text-left text-sm">
			<thead class="text-zinc-400">
				<tr>
					<th class="py-1 font-normal">Member</th>
					<th class="font-normal">Role</th>
					<th class="font-normal">Joined</th>
					<th class="font-normal"><span class="sr-only">Actions</span></th>
				</tr>
			</thead>
			<tbody>
				{#each team.members as m (m.user.id)}
					<tr class="border-t border-zinc-800">
						<td class="py-1.5">
							<div>
								{m.user.displayName}
								{#if isMe(m)}<span class="text-xs text-zinc-500">(you)</span>{/if}
							</div>
							<div class="text-xs text-zinc-500">@{m.user.username}</div>
						</td>
						<td>
							{#if canChange(m)}
								<select
									class="rounded border border-zinc-700 bg-zinc-900 px-2 py-1 text-sm"
									aria-label="Role of {m.user.username}"
									value={m.role}
									onchange={(e) =>
										run(() =>
											server.setMemberRole(id, m.user.id, e.currentTarget.value as TeamRole)
										)}
								>
									{#each roles as r (r)}
										<option value={r}>{r}</option>
									{/each}
								</select>
							{:else}
								{m.role}
							{/if}
						</td>
						<td class="text-xs text-zinc-400">{formatDateTime(m.joinedAt)}</td>
						<td class="text-right">
							{#if canChange(m) && !isMe(m)}
								<button
									type="button"
									class={small}
									onclick={() =>
										run(
											() => server.removeMember(id, m.user.id),
											`Remove @${m.user.username} from the team? They lose access to what is shared with it.`
										)}
								>
									Remove
								</button>
							{/if}
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
		<p class="mt-3 text-xs text-zinc-500">
			Members see what is shared with the team. Admins also invite and remove members. Owners can
			also rename and delete the team and appoint other owners.
		</p>
	</section>

	{#if canManage}
		<section class="mb-6 max-w-4xl rounded border border-zinc-800 p-4">
			<h2 class="mb-3 font-medium">Invite</h2>
			<form class="flex flex-wrap items-end gap-2" onsubmit={invite}>
				<label class="flex flex-col gap-1 text-sm">
					<span class="text-zinc-400">Username</span>
					<input class={input} bind:value={inviteName} required maxlength="33" />
				</label>
				<button type="submit" class={button} disabled={!inviteName.trim()}>Invite</button>
				{#if inviteMsg}<span class="pb-1.5 text-sm text-zinc-300" role="status">
						{inviteMsg}
					</span>{/if}
			</form>

			<form class="mt-4 flex flex-wrap items-end gap-2" onsubmit={createLink}>
				<label class="flex flex-col gap-1 text-sm">
					<span class="text-zinc-400">Link valid for (days)</span>
					<input class="{input} w-28" type="number" min="1" max="365" bind:value={linkDays} />
				</label>
				<label class="flex flex-col gap-1 text-sm">
					<span class="text-zinc-400">Uses (empty = unlimited)</span>
					<input class="{input} w-28" type="number" min="1" max="10000" bind:value={linkUses} />
				</label>
				<button type="submit" class={plain}>Create invite link</button>
			</form>
			{#if newLink}
				<div
					class="mt-3 rounded border border-emerald-800 bg-emerald-950 p-3 text-sm text-emerald-200"
					role="status"
				>
					Send this link to the people you want in the team (shown only now). Anyone with an account
					on this server who has it can join:
					<div class="mt-2 flex flex-wrap items-center gap-2">
						<code class="font-mono break-all text-emerald-100 select-all">{newLink}</code>
						<button type="button" class={small} onclick={copyLink}>
							{copied ? 'Copied' : 'Copy'}
						</button>
					</div>
				</div>
			{/if}

			{#if team.invites.length > 0}
				<h3 class="mt-4 mb-1 text-sm text-zinc-400">Open invitations</h3>
				<ul class="flex flex-col text-sm">
					{#each team.invites as i (i.id)}
						<li class="flex items-center justify-between gap-2 border-t border-zinc-800 py-1.5">
							<span>
								{inviteText(i)}
								{#if i.invitedBy}
									<span class="text-xs text-zinc-500">by @{i.invitedBy.username}</span>
								{/if}
							</span>
							<button
								type="button"
								class={small}
								onclick={() => run(() => server.revokeInvite(id, i.id))}
							>
								Withdraw
							</button>
						</li>
					{/each}
				</ul>
			{/if}
		</section>
	{/if}
{/if}
