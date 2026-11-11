<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import {
		accountApi,
		type AdminUser,
		type AuditEntry,
		type ServerSettings,
		type UserUpdate
	} from '#lib/api/index.ts';
	import { auth, errorText } from '#lib/app-state.svelte.ts';
	import { formatDateTime } from '#lib/format.ts';

	const account = accountApi();

	let users = $state<AdminUser[]>([]);
	let settings = $state<ServerSettings | null>(null);
	let audit = $state<AuditEntry[]>([]);
	let error = $state<string | null>(null);
	/** Shown once after creating or resetting an account */
	let handout = $state<{ username: string; password: string } | null>(null);

	let newUsername = $state('');
	let newDisplayName = $state('');
	let newIsAdmin = $state(false);
	let settingsMsg = $state<string | null>(null);

	async function load() {
		if (!account) return;
		try {
			[users, settings, audit] = await Promise.all([
				account.adminUsers(),
				account.adminSettings(),
				account.adminAudit(100)
			]);
			error = null;
		} catch (e) {
			error = errorText(e);
		}
	}
	if (account && auth.me?.isAdmin) load();

	async function create(event: SubmitEvent) {
		event.preventDefault();
		try {
			const r = await account!.adminCreateUser(newUsername, newDisplayName, newIsAdmin);
			handout = { username: newUsername.trim().toLowerCase(), password: r.temporaryPassword ?? '' };
			newUsername = newDisplayName = '';
			newIsAdmin = false;
			await load();
		} catch (e) {
			error = errorText(e);
		}
	}

	async function update(u: AdminUser, change: UserUpdate, question?: string) {
		if (question && !confirm(question)) return;
		try {
			const r = await account!.adminUpdateUser(u.id, change);
			if (r.temporaryPassword) handout = { username: u.username, password: r.temporaryPassword };
			await load();
		} catch (e) {
			error = errorText(e);
		}
	}

	async function saveSettings(event: SubmitEvent) {
		event.preventDefault();
		if (!settings) return;
		try {
			settings = await account!.adminSaveSettings(settings);
			settingsMsg = 'Saved.';
			await load();
		} catch (e) {
			settingsMsg = errorText(e);
		}
	}

	const input =
		'rounded border border-zinc-700 bg-zinc-900 px-3 py-1.5 text-sm focus:border-zinc-400 focus:outline-none';
	const button =
		'rounded bg-zinc-100 px-3 py-1.5 text-sm font-medium text-zinc-900 hover:bg-white disabled:opacity-50';
	const small = 'rounded border border-zinc-700 px-2 py-0.5 text-xs hover:bg-zinc-800';
</script>

<h1 class="mb-4 text-2xl font-semibold">Administration</h1>

{#if !auth.me?.isAdmin}
	<p class="text-zinc-400">Admins only.</p>
{:else}
	{#if error}
		<p class="mb-4 text-sm text-red-400" role="alert">{error}</p>
	{/if}
	{#if handout}
		<div
			class="mb-4 max-w-3xl rounded border border-emerald-800 bg-emerald-950 p-3 text-sm text-emerald-200"
			role="status"
		>
			Temporary password for <strong>@{handout.username}</strong> (shown once; they must change it
			at the first login):
			<code class="ml-1 font-mono text-emerald-100 select-all">{handout.password}</code>
			<button type="button" class="ml-3 text-xs opacity-70" onclick={() => (handout = null)}
				>Dismiss</button
			>
		</div>
	{/if}

	<section class="mb-6 max-w-4xl rounded border border-zinc-800 p-4">
		<h2 class="mb-3 font-medium">Users</h2>
		<table class="mb-4 w-full text-left text-sm">
			<thead class="text-zinc-400">
				<tr>
					<th class="py-1 font-normal">User</th>
					<th class="font-normal">Status</th>
					<th class="font-normal">Created</th>
					<th class="font-normal"><span class="sr-only">Actions</span></th>
				</tr>
			</thead>
			<tbody>
				{#each users as u (u.id)}
					{@const self = u.id === auth.me.id}
					<tr class="border-t border-zinc-800" class:opacity-50={u.disabled}>
						<td class="py-1.5">
							<div>{u.displayName}</div>
							<div class="text-xs text-zinc-500">@{u.username}</div>
						</td>
						<td class="text-xs">
							{#if u.isAdmin}<span class="mr-1 rounded bg-purple-900 px-1.5 text-purple-200"
									>admin</span
								>{/if}
							{#if u.disabled}<span class="mr-1 rounded bg-red-950 px-1.5 text-red-300"
									>disabled</span
								>{/if}
							{#if u.totpEnabled}<span class="mr-1 rounded bg-zinc-800 px-1.5">TOTP</span>{/if}
							{#if u.hasPasskey}<span class="mr-1 rounded bg-zinc-800 px-1.5">passkey</span>{/if}
							{#if u.mustChangePassword}<span class="rounded bg-amber-950 px-1.5 text-amber-300"
									>temporary password</span
								>{/if}
						</td>
						<td class="text-xs text-zinc-400">{formatDateTime(u.createdAt)}</td>
						<td class="text-right whitespace-nowrap">
							{#if !self}
								<button
									type="button"
									class={small}
									onclick={() =>
										update(
											u,
											{ disabled: !u.disabled },
											u.disabled ? undefined : `Disable @${u.username}? They are logged out.`
										)}>{u.disabled ? 'Enable' : 'Disable'}</button
								>
								<button
									type="button"
									class={small}
									onclick={() => update(u, { isAdmin: !u.isAdmin })}
									>{u.isAdmin ? 'Remove admin' : 'Make admin'}</button
								>
							{/if}
							<button
								type="button"
								class={small}
								onclick={() =>
									update(
										u,
										{ resetPassword: true },
										`Set a new temporary password for @${u.username}? They are logged out.`
									)}>Reset password</button
							>
							{#if u.totpEnabled || u.hasPasskey}
								<button
									type="button"
									class={small}
									onclick={() =>
										update(
											u,
											{ resetTwoFactor: true },
											`Remove two-factor authentication (TOTP and passkeys) of @${u.username}?`
										)}>Reset 2FA</button
								>
							{/if}
						</td>
					</tr>
				{/each}
			</tbody>
		</table>

		<form class="flex flex-wrap items-end gap-2" onsubmit={create}>
			<label class="flex flex-col gap-1 text-sm">
				<span class="text-zinc-400">Username</span>
				<input class={input} bind:value={newUsername} required minlength="3" maxlength="32" />
			</label>
			<label class="flex flex-col gap-1 text-sm">
				<span class="text-zinc-400">Display name (optional)</span>
				<input class={input} bind:value={newDisplayName} maxlength="64" />
			</label>
			<label class="flex items-center gap-2 pb-2 text-sm">
				<input type="checkbox" bind:checked={newIsAdmin} /> Admin
			</label>
			<button type="submit" class={button}>Create user</button>
		</form>
	</section>

	{#if settings}
		<section class="mb-6 max-w-4xl rounded border border-zinc-800 p-4">
			<h2 class="mb-3 font-medium">Server settings</h2>
			<form class="flex flex-col gap-3 text-sm" onsubmit={saveSettings}>
				<fieldset class="flex flex-col gap-1">
					<legend class="mb-1 text-zinc-400">Who can create an account</legend>
					<label class="flex items-center gap-2">
						<input type="radio" value="closed" bind:group={settings.registration} />
						Only admins (closed)
					</label>
					<label class="flex items-center gap-2">
						<input type="radio" value="open" bind:group={settings.registration} />
						Anyone who can reach this server (open)
					</label>
				</fieldset>
				<label class="flex items-start gap-2">
					<input type="checkbox" bind:checked={settings.enforce2fa} class="mt-1" />
					<span>
						Require two-factor authentication for everyone
						<span class="block text-xs text-zinc-500"
							>Users without it must set it up before they can do anything else.</span
						>
					</span>
				</label>
				<label class="flex items-start gap-2">
					<input type="checkbox" bind:checked={settings.allowShareEveryone} class="mt-1" />
					<span>
						Allow sharing with "everyone on this server"
						<span class="block text-xs text-zinc-500"
							>When off, nobody can choose it, and what was shared with everyone is not visible to
							others until it is turned on again.</span
						>
					</span>
				</label>
				<div class="flex items-center gap-3">
					<button type="submit" class={button}>Save</button>
					{#if settingsMsg}<span class="text-zinc-300">{settingsMsg}</span>{/if}
				</div>
			</form>
		</section>
	{/if}

	<section class="max-w-4xl rounded border border-zinc-800 p-4">
		<h2 class="mb-3 font-medium">Security log</h2>
		<table class="w-full text-left text-xs">
			<thead class="text-zinc-400">
				<tr>
					<th class="py-1 font-normal">When</th>
					<th class="font-normal">User</th>
					<th class="font-normal">Event</th>
					<th class="font-normal">Address</th>
					<th class="font-normal">Details</th>
				</tr>
			</thead>
			<tbody>
				{#each audit as a, i (i)}
					<tr class="border-t border-zinc-800">
						<td class="py-1 whitespace-nowrap text-zinc-400">{formatDateTime(a.at)}</td>
						<td>{a.username ?? '–'}</td>
						<td class:text-amber-300={a.event.includes('failed') || a.event.includes('limited')}
							>{a.event}</td
						>
						<td class="text-zinc-400">{a.ip ?? ''}</td>
						<td class="max-w-64 truncate text-zinc-500" title={JSON.stringify(a.details)}>
							{Object.keys(a.details).length ? JSON.stringify(a.details) : ''}
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</section>
{/if}
