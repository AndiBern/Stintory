<!-- SPDX-FileCopyrightText: 2026 The Rundenzeit Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { accountApi, ApiError, type LoginResponse } from '#lib/api/index.ts';
	import { auth, errorText } from '#lib/app-state.svelte.ts';

	const account = accountApi();

	let mode = $state<'login' | 'register'>('login');
	let username = $state('');
	let password = $state('');
	let displayName = $state('');
	let code = $state('');
	let challenge = $state<string | null>(null);
	let error = $state<string | null>(null);
	let busy = $state(false);

	/** Where to go after logging in: only paths on this site. */
	function target(): string {
		const next = page.url.searchParams.get('next') ?? '';
		return next.startsWith('/') && !next.startsWith('//') && !next.includes('\\')
			? next
			: resolve('/');
	}

	// already logged in (e.g. back button)
	$effect(() => {
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- checked local path
		if (auth.me) goto(target());
	});

	function done(r: LoginResponse) {
		if (r.status === 'secondFactor') {
			challenge = r.challenge;
			code = '';
			return;
		}
		auth.me = r.user;
		password = '';
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- checked local path
		goto(target());
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (!account || busy) return;
		busy = true;
		error = null;
		try {
			if (challenge) {
				done(await account.loginSecondFactor(challenge, code));
			} else if (mode === 'register') {
				done(await account.register(username, password, displayName));
			} else {
				done(await account.login(username, password));
			}
		} catch (e) {
			if (e instanceof ApiError && e.code === 'challenge_expired') {
				challenge = null;
				password = '';
			}
			error = errorText(e);
		} finally {
			busy = false;
		}
	}

	const input =
		'w-full rounded border border-zinc-700 bg-zinc-900 px-3 py-2 text-sm focus:border-zinc-400 focus:outline-none';
</script>

<div class="mx-auto mt-16 max-w-sm">
	<h1 class="mb-6 text-2xl font-semibold">
		{challenge ? 'Two-factor authentication' : mode === 'register' ? 'Create account' : 'Log in'}
	</h1>

	{#if !account}
		<p class="text-zinc-400">Logging in is only needed on a Rundenzeit server.</p>
	{:else}
		<form class="flex flex-col gap-4" onsubmit={submit}>
			{#if challenge}
				<label class="flex flex-col gap-1 text-sm">
					<span class="text-zinc-400">Code from your authenticator app, or a recovery code</span>
					<!-- svelte-ignore a11y_autofocus -->
					<input
						class="{input} font-mono tracking-widest"
						bind:value={code}
						autocomplete="one-time-code"
						inputmode="text"
						required
						autofocus
					/>
				</label>
			{:else}
				<label class="flex flex-col gap-1 text-sm">
					<span class="text-zinc-400">Username</span>
					<input class={input} bind:value={username} autocomplete="username" required />
				</label>
				{#if mode === 'register'}
					<label class="flex flex-col gap-1 text-sm">
						<span class="text-zinc-400">Display name (optional)</span>
						<input class={input} bind:value={displayName} autocomplete="nickname" />
					</label>
				{/if}
				<label class="flex flex-col gap-1 text-sm">
					<span class="text-zinc-400">Password</span>
					<input
						class={input}
						type="password"
						bind:value={password}
						autocomplete={mode === 'register' ? 'new-password' : 'current-password'}
						minlength={mode === 'register' ? 12 : undefined}
						required
					/>
					{#if mode === 'register'}
						<span class="text-xs text-zinc-500">At least 12 characters.</span>
					{/if}
				</label>
			{/if}

			{#if error}
				<p class="text-sm text-red-400" role="alert">{error}</p>
			{/if}

			<button
				type="submit"
				class="rounded bg-zinc-100 px-3 py-2 text-sm font-medium text-zinc-900 hover:bg-white disabled:opacity-50"
				disabled={busy}
			>
				{challenge ? 'Verify' : mode === 'register' ? 'Create account' : 'Log in'}
			</button>

			{#if challenge}
				<button
					type="button"
					class="text-sm text-zinc-400 hover:text-zinc-200"
					onclick={() => {
						challenge = null;
						error = null;
					}}>Back</button
				>
			{:else if auth.registrationOpen}
				<button
					type="button"
					class="text-sm text-zinc-400 hover:text-zinc-200"
					onclick={() => {
						mode = mode === 'login' ? 'register' : 'login';
						error = null;
					}}
				>
					{mode === 'login' ? 'No account yet? Create one' : 'Have an account? Log in'}
				</button>
			{/if}
		</form>
	{/if}
</div>
