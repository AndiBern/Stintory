<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { api, type Grantee, type TeamSummary, type UserRef } from '#lib/api/index.ts';
	import { errorText } from '#lib/app-state.svelte.ts';

	let {
		grants = $bindable([]),
		everyoneAllowed
	}: {
		/** Who it is shared with: everyone, teams and people */
		grants: Grantee[];
		/** The admin allows sharing with everyone */
		everyoneAllowed: boolean;
	} = $props();

	const server = api().server;
	const uid = $props.id();

	let teams = $state<TeamSummary[]>([]);
	let username = $state('');
	let suggestions = $state<UserRef[]>([]);
	let error = $state<string | null>(null);

	server
		.teams()
		.then((t) => (teams = t))
		.catch(() => {});

	const everyone = $derived(grants.some((g) => g.kind === 'everyone'));
	const people = $derived(grants.filter((g) => g.kind === 'user'));

	function teamGrant(id: string): Grantee | undefined {
		return grants.find((g) => g.kind === 'team' && g.id === id);
	}

	/** The car setups are shared too (users and teams only). */
	function withSetup(g: Grantee | undefined): boolean {
		return !!g && g.kind !== 'everyone' && !!g.withSetup;
	}

	function setSetup(kind: 'user' | 'team', id: string, on: boolean) {
		grants = grants.map((g) => (g.kind === kind && g.id === id ? { ...g, withSetup: on } : g));
	}

	function setEveryone(on: boolean) {
		grants = on
			? [...grants.filter((g) => g.kind !== 'everyone'), { kind: 'everyone' }]
			: grants.filter((g) => g.kind !== 'everyone');
	}

	function setTeam(team: TeamSummary, on: boolean) {
		const others = grants.filter((g) => !(g.kind === 'team' && g.id === team.id));
		grants = on ? [...others, { kind: 'team', id: team.id, name: team.name }] : others;
	}

	function removeUser(id: string) {
		grants = grants.filter((g) => !(g.kind === 'user' && g.id === id));
	}

	async function suggest() {
		const q = username.trim();
		if (q.length < 2) {
			suggestions = [];
			return;
		}
		try {
			const found = await server.suggestUsers(q);
			if (username.trim() === q) suggestions = found;
		} catch {
			// suggestions are optional
		}
	}

	async function addUser() {
		const name = username.trim().replace(/^@/, '');
		if (!name) return;
		error = null;
		try {
			const u = await server.lookupUser(name);
			if (!grants.some((g) => g.kind === 'user' && g.id === u.id)) {
				grants = [...grants, { kind: 'user', ...u }];
			}
			username = '';
			suggestions = [];
		} catch (e) {
			error = errorText(e);
		}
	}
</script>

<div class="flex flex-col gap-3 text-sm">
	<label class="flex items-start gap-2" class:opacity-50={!everyoneAllowed}>
		<input
			type="checkbox"
			class="mt-1"
			checked={everyone && everyoneAllowed}
			disabled={!everyoneAllowed}
			onchange={(e) => setEveryone(e.currentTarget.checked)}
		/>
		<span>
			Everyone on this server
			<span class="block text-xs text-zinc-400">
				{everyoneAllowed
					? 'All accounts, including ones created later.'
					: 'Turned off by the admin of this server.'}
			</span>
		</span>
	</label>

	<p class="text-xs text-zinc-500">
		Car setups are shared only with the teams and people marked “Setups too”, never with everyone.
	</p>

	<fieldset class="flex flex-col gap-1">
		<legend class="mb-1 text-zinc-400">Teams</legend>
		{#each teams as team (team.id)}
			{@const grant = teamGrant(team.id)}
			<div class="flex flex-wrap items-center justify-between gap-x-3">
				<label class="flex items-center gap-2">
					<input
						type="checkbox"
						checked={!!grant}
						onchange={(e) => setTeam(team, e.currentTarget.checked)}
					/>
					{team.name}
					<span class="text-xs text-zinc-500">{team.memberCount} members</span>
				</label>
				{#if grant}
					<label class="flex items-center gap-1.5 text-xs text-zinc-400">
						<input
							type="checkbox"
							checked={withSetup(grant)}
							onchange={(e) => setSetup('team', team.id, e.currentTarget.checked)}
						/>
						Setups too
					</label>
				{/if}
			</div>
		{:else}
			<p class="text-xs text-zinc-500">You are not in a team yet.</p>
		{/each}
	</fieldset>

	<div class="flex flex-col gap-1">
		<span class="text-zinc-400">People</span>
		{#each people as p (p.id)}
			<div class="flex items-center justify-between gap-2">
				<span class="min-w-0 flex-1">
					{p.displayName} <span class="text-xs text-zinc-500">@{p.username}</span>
				</span>
				<label class="flex items-center gap-1.5 text-xs text-zinc-400">
					<input
						type="checkbox"
						checked={withSetup(p)}
						onchange={(e) => setSetup('user', p.id, e.currentTarget.checked)}
					/>
					Setups too
				</label>
				<button
					type="button"
					class="rounded border border-zinc-700 px-2 py-0.5 text-xs hover:bg-zinc-800"
					aria-label="Remove {p.username}"
					onclick={() => removeUser(p.id)}
				>
					Remove
				</button>
			</div>
		{/each}
		<div class="flex gap-2">
			<input
				type="text"
				class="min-w-0 flex-1 rounded border border-zinc-700 bg-zinc-950 px-2 py-1"
				placeholder="Username"
				aria-label="Username to share with"
				maxlength="33"
				autocomplete="off"
				list="{uid}-users"
				bind:value={username}
				oninput={suggest}
				onkeydown={(e) => {
					if (e.key === 'Enter') {
						e.preventDefault();
						void addUser();
					}
				}}
			/>
			<datalist id="{uid}-users">
				{#each suggestions as u (u.id)}
					<option value={u.username}>{u.displayName}</option>
				{/each}
			</datalist>
			<button
				type="button"
				class="rounded border border-zinc-700 px-3 py-1 hover:bg-zinc-800 disabled:opacity-40"
				disabled={!username.trim()}
				onclick={addUser}
			>
				Add
			</button>
		</div>
		{#if error}<p class="text-xs text-red-400" role="alert">{error}</p>{/if}
	</div>
</div>
