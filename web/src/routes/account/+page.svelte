<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import {
		accountApi,
		type BrowserSession,
		type DeviceInfo,
		type PasskeyInfo,
		type TotpSetup
	} from '#lib/api/index.ts';
	import { auth, errorText, refreshAuth } from '#lib/app-state.svelte.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { passkeyErrorText, passkeysSupported } from '#lib/webauthn.ts';

	const account = accountApi();

	// profile
	let displayName = $state(auth.me?.displayName ?? '');
	let profileMsg = $state<string | null>(null);

	// password
	let currentPassword = $state('');
	let newPassword = $state('');
	let newPassword2 = $state('');
	let passwordMsg = $state<{ ok: boolean; text: string } | null>(null);

	// two-factor
	let setup = $state<TotpSetup | null>(null);
	let setupCode = $state('');
	let recoveryCodes = $state<string[] | null>(null);
	let confirmPassword = $state('');
	let totpMsg = $state<{ ok: boolean; text: string } | null>(null);

	let sessions = $state<BrowserSession[]>([]);

	async function loadSessions() {
		sessions = (await account?.browserSessions().catch(() => [])) ?? [];
	}
	if (account) loadSessions();

	// passkeys
	let passkeys = $state<PasskeyInfo[]>([]);
	let passkeyName = $state('');
	let passkeyPassword = $state('');
	let passkeyMsg = $state<{ ok: boolean; text: string } | null>(null);
	const canPasskey = passkeysSupported();

	async function loadPasskeys() {
		passkeys = (await account?.passkeys().catch(() => [])) ?? [];
	}
	if (account) loadPasskeys();

	async function addPasskey(event: SubmitEvent) {
		event.preventDefault();
		passkeyMsg = null;
		try {
			const p = await account!.addPasskey(passkeyPassword, passkeyName);
			passkeyName = passkeyPassword = '';
			passkeyMsg = { ok: true, text: `Passkey "${p.name}" added.` };
			await loadPasskeys();
			await refreshAuth();
		} catch (e) {
			passkeyMsg = {
				ok: false,
				text: e instanceof Error && e.name === 'ApiError' ? e.message : passkeyErrorText(e)
			};
		}
	}

	function removePasskey(p: PasskeyInfo) {
		if (
			!confirm(`Remove the passkey "${p.name}"? Remove it from your device's password manager too.`)
		)
			return;
		run(
			async () => {
				await account!.removePasskey(p.id);
				await loadPasskeys();
				await refreshAuth();
			},
			(t) => (passkeyMsg = { ok: false, text: t })
		);
	}

	// desktop apps
	let devices = $state<DeviceInfo[]>([]);
	let newDeviceName = $state('');
	let newToken = $state<string | null>(null);
	let deviceMsg = $state<string | null>(null);

	async function loadDevices() {
		devices = (await account?.devices().catch(() => [])) ?? [];
	}
	if (account) loadDevices();

	function revokeDevice(d: DeviceInfo) {
		if (!confirm(`Disconnect "${d.name}"? It can't sync anymore until it connects again.`)) return;
		run(
			async () => {
				await account!.revokeDevice(d.id);
				await loadDevices();
			},
			(t) => (deviceMsg = t)
		);
	}

	function createToken(event: SubmitEvent) {
		event.preventDefault();
		run(
			async () => {
				newToken = (await account!.createDevice(newDeviceName)).token;
				newDeviceName = '';
				await loadDevices();
			},
			(t) => (deviceMsg = t)
		);
	}

	async function run(action: () => Promise<void>, onError: (text: string) => void) {
		try {
			await action();
		} catch (e) {
			onError(errorText(e));
		}
	}

	function saveProfile(event: SubmitEvent) {
		event.preventDefault();
		run(
			async () => {
				await account!.updateMe(displayName);
				await refreshAuth();
				profileMsg = 'Saved.';
			},
			(t) => (profileMsg = t)
		);
	}

	function changePassword(event: SubmitEvent) {
		event.preventDefault();
		if (newPassword !== newPassword2) {
			passwordMsg = { ok: false, text: 'The new passwords differ.' };
			return;
		}
		run(
			async () => {
				await account!.changePassword(currentPassword, newPassword);
				currentPassword = newPassword = newPassword2 = '';
				passwordMsg = { ok: true, text: 'Password changed. Other sessions were logged out.' };
				await refreshAuth();
				await loadSessions();
			},
			(t) => (passwordMsg = { ok: false, text: t })
		);
	}

	function startSetup() {
		totpMsg = null;
		run(
			async () => {
				setup = await account!.totpSetup();
				setupCode = '';
			},
			(t) => (totpMsg = { ok: false, text: t })
		);
	}

	function confirmSetup(event: SubmitEvent) {
		event.preventDefault();
		run(
			async () => {
				recoveryCodes = (await account!.totpConfirm(setupCode)).recoveryCodes;
				setup = null;
				await refreshAuth();
			},
			(t) => (totpMsg = { ok: false, text: t })
		);
	}

	function regenerate() {
		run(
			async () => {
				recoveryCodes = (await account!.regenerateRecoveryCodes(confirmPassword)).recoveryCodes;
				confirmPassword = '';
				totpMsg = null;
			},
			(t) => (totpMsg = { ok: false, text: t })
		);
	}

	function disable() {
		if (!confirm('Turn off two-factor authentication?')) return;
		run(
			async () => {
				await account!.totpDisable(confirmPassword);
				confirmPassword = '';
				recoveryCodes = null;
				totpMsg = { ok: true, text: 'Two-factor authentication is off.' };
				await refreshAuth();
			},
			(t) => (totpMsg = { ok: false, text: t })
		);
	}

	function downloadCodes() {
		if (!recoveryCodes) return;
		const text = `Stintory recovery codes for ${auth.me?.username} (${location.host})\nEach code works once.\n\n${recoveryCodes.join('\n')}\n`;
		const a = document.createElement('a');
		a.href = URL.createObjectURL(new Blob([text], { type: 'text/plain' }));
		a.download = 'stintory-recovery-codes.txt';
		a.click();
		URL.revokeObjectURL(a.href);
	}

	function endSession(id: string) {
		run(
			async () => {
				await account!.endBrowserSession(id);
				await loadSessions();
			},
			() => {}
		);
	}

	const input =
		'w-full rounded border border-zinc-700 bg-zinc-900 px-3 py-1.5 text-sm focus:border-zinc-400 focus:outline-none';
	const button =
		'rounded bg-zinc-100 px-3 py-1.5 text-sm font-medium text-zinc-900 hover:bg-white disabled:opacity-50';
	const secondary = 'rounded border border-zinc-700 px-3 py-1.5 text-sm hover:bg-zinc-800';
</script>

<h1 class="mb-4 text-2xl font-semibold">Account</h1>

{#if !account || !auth.me}
	<p class="text-zinc-400">Accounts exist on a Stintory server.</p>
{:else}
	{#if auth.me.mustChangePassword}
		<p
			class="mb-4 max-w-2xl rounded border border-amber-800 bg-amber-950 p-3 text-sm text-amber-200"
		>
			Your password was set by an admin. Please choose your own password to continue.
		</p>
	{:else if auth.me.twoFactorRequired}
		<p
			class="mb-4 max-w-2xl rounded border border-amber-800 bg-amber-950 p-3 text-sm text-amber-200"
		>
			This server requires two-factor authentication. Please set it up to continue.
		</p>
	{/if}

	{#if !auth.me.mustChangePassword}
		<section class="mb-6 max-w-2xl rounded border border-zinc-800 p-4">
			<h2 class="mb-3 font-medium">Profile</h2>
			<form class="flex items-end gap-2" onsubmit={saveProfile}>
				<label class="flex flex-1 flex-col gap-1 text-sm">
					<span class="text-zinc-400">Display name (shown to people you share with)</span>
					<input class={input} bind:value={displayName} maxlength="64" required />
				</label>
				<button type="submit" class={button}>Save</button>
			</form>
			<p class="mt-2 text-xs text-zinc-500">Username: @{auth.me.username}</p>
			{#if profileMsg}<p class="mt-2 text-sm text-zinc-300">{profileMsg}</p>{/if}
		</section>
	{/if}

	<section class="mb-6 max-w-2xl rounded border border-zinc-800 p-4">
		<h2 class="mb-3 font-medium">Password</h2>
		<form class="grid gap-3" onsubmit={changePassword}>
			<label class="flex flex-col gap-1 text-sm">
				<span class="text-zinc-400">Current password</span>
				<input
					class={input}
					type="password"
					bind:value={currentPassword}
					autocomplete="current-password"
					required
				/>
			</label>
			<label class="flex flex-col gap-1 text-sm">
				<span class="text-zinc-400">New password (at least 12 characters)</span>
				<input
					class={input}
					type="password"
					bind:value={newPassword}
					autocomplete="new-password"
					minlength="12"
					required
				/>
			</label>
			<label class="flex flex-col gap-1 text-sm">
				<span class="text-zinc-400">New password again</span>
				<input
					class={input}
					type="password"
					bind:value={newPassword2}
					autocomplete="new-password"
					minlength="12"
					required
				/>
			</label>
			<div><button type="submit" class={button}>Change password</button></div>
			{#if passwordMsg}
				<p class="text-sm" class:text-red-400={!passwordMsg.ok} role="status">
					{passwordMsg.text}
				</p>
			{/if}
		</form>
	</section>

	{#if !auth.me.mustChangePassword}
		<section class="mb-6 max-w-2xl rounded border border-zinc-800 p-4">
			<h2 class="mb-1 font-medium">Two-factor authentication</h2>
			<p class="mb-3 text-sm text-zinc-400">
				A code from an authenticator app (e.g. Aegis, 2FAS, Google Authenticator) in addition to
				your password.
			</p>

			{#if recoveryCodes}
				<div class="mb-3 rounded border border-emerald-800 bg-emerald-950 p-3 text-sm">
					<p class="mb-2 text-emerald-200">
						Save these recovery codes somewhere safe. Each works once if you lose your
						authenticator. They won't be shown again.
					</p>
					<ul class="mb-3 grid grid-cols-2 gap-1 font-mono text-emerald-100">
						{#each recoveryCodes as c (c)}<li>{c}</li>{/each}
					</ul>
					<div class="flex gap-2">
						<button type="button" class={secondary} onclick={downloadCodes}>Download</button>
						<button type="button" class={secondary} onclick={() => (recoveryCodes = null)}
							>I saved them</button
						>
					</div>
				</div>
			{/if}

			{#if auth.me.totpEnabled}
				<p class="mb-3 text-sm text-emerald-300">✓ Enabled</p>
				<div class="flex flex-wrap items-end gap-2">
					<label class="flex flex-1 flex-col gap-1 text-sm">
						<span class="text-zinc-400">Password, to confirm</span>
						<input
							class={input}
							type="password"
							bind:value={confirmPassword}
							autocomplete="current-password"
						/>
					</label>
					<button type="button" class={secondary} disabled={!confirmPassword} onclick={regenerate}
						>New recovery codes</button
					>
					<button
						type="button"
						class="{secondary} hover:border-red-700 hover:text-red-300"
						disabled={!confirmPassword}
						onclick={disable}>Turn off</button
					>
				</div>
			{:else if setup}
				<div class="flex flex-wrap gap-4">
					<img
						src="data:image/svg+xml;charset=utf-8,{encodeURIComponent(setup.qrSvg)}"
						alt="QR code for your authenticator app"
						class="h-48 w-48 rounded bg-white p-1"
					/>
					<form class="flex min-w-60 flex-1 flex-col gap-3" onsubmit={confirmSetup}>
						<p class="text-sm text-zinc-300">
							1. Scan the QR code with your authenticator app, or enter the key:
						</p>
						<code class="text-xs break-all text-zinc-300 select-all">{setup.secret}</code>
						<label class="flex flex-col gap-1 text-sm">
							<span class="text-zinc-300">2. Enter the 6-digit code it shows</span>
							<input
								class="{input} font-mono tracking-widest"
								bind:value={setupCode}
								inputmode="numeric"
								autocomplete="one-time-code"
								maxlength="6"
								required
							/>
						</label>
						<div class="flex gap-2">
							<button type="submit" class={button}>Turn on</button>
							<button type="button" class={secondary} onclick={() => (setup = null)}>Cancel</button>
						</div>
					</form>
				</div>
			{:else}
				<button type="button" class={button} onclick={startSetup}>Set up</button>
			{/if}
			{#if totpMsg}
				<p class="mt-2 text-sm" class:text-red-400={!totpMsg.ok} role="status">{totpMsg.text}</p>
			{/if}
		</section>

		<section class="mb-6 max-w-2xl rounded border border-zinc-800 p-4">
			<h2 class="mb-1 font-medium">Passkeys</h2>
			<p class="mb-3 text-sm text-zinc-400">
				Log in with your fingerprint, face or device PIN instead of a password (phone, laptop,
				password manager or security key). A passkey also counts as two-factor authentication.
			</p>
			{#if passkeys.length > 0}
				<table class="mb-3 w-full text-left text-sm">
					<tbody>
						{#each passkeys as p (p.id)}
							<tr class="border-t border-zinc-800 first:border-t-0">
								<td class="py-1.5">
									<div class="text-zinc-300">{p.name}</div>
									<div class="text-xs text-zinc-500">
										added {formatDateTime(p.createdAt)} · last used {p.lastUsedAt
											? formatDateTime(p.lastUsedAt)
											: 'never'}
									</div>
								</td>
								<td class="text-right">
									<button type="button" class={secondary} onclick={() => removePasskey(p)}
										>Remove</button
									>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			{/if}
			{#if canPasskey}
				<form class="flex flex-wrap items-end gap-2" onsubmit={addPasskey}>
					<label class="flex min-w-40 flex-1 flex-col gap-1 text-sm">
						<span class="text-zinc-400">Name (e.g. "Phone")</span>
						<input class={input} bind:value={passkeyName} maxlength="64" />
					</label>
					<label class="flex min-w-40 flex-1 flex-col gap-1 text-sm">
						<span class="text-zinc-400">Your password, to confirm</span>
						<input
							class={input}
							type="password"
							bind:value={passkeyPassword}
							autocomplete="current-password"
							required
						/>
					</label>
					<button type="submit" class={button}>Add passkey</button>
				</form>
			{:else}
				<p class="text-sm text-zinc-500">This browser doesn't support passkeys.</p>
			{/if}
			{#if passkeyMsg}
				<p class="mt-2 text-sm" class:text-red-400={!passkeyMsg.ok} role="status">
					{passkeyMsg.text}
				</p>
			{/if}
		</section>

		<section class="max-w-2xl rounded border border-zinc-800 p-4">
			<h2 class="mb-3 font-medium">Logged-in browsers</h2>
			<table class="w-full text-left text-sm">
				<tbody>
					{#each sessions as s (s.id)}
						<tr class="border-t border-zinc-800 first:border-t-0">
							<td class="py-1.5">
								<div class="truncate text-zinc-300" title={s.userAgent ?? ''}>
									{s.userAgent ?? 'Unknown browser'}
								</div>
								<div class="text-xs text-zinc-500">
									{s.ip ?? ''} · last used {formatDateTime(s.lastSeenAt)}
								</div>
							</td>
							<td class="text-right">
								{#if s.current}
									<span class="text-xs text-zinc-400">this browser</span>
								{:else}
									<button type="button" class={secondary} onclick={() => endSession(s.id)}
										>Log out</button
									>
								{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</section>

		<section class="mt-6 max-w-2xl rounded border border-zinc-800 p-4">
			<h2 class="mb-1 font-medium">Desktop apps</h2>
			<p class="mb-3 text-sm text-zinc-400">
				Computers that sync with your account. Connect one from the app's settings ("Server sync").
			</p>
			{#if devices.length === 0}
				<p class="mb-3 text-sm text-zinc-500">None connected.</p>
			{:else}
				<table class="mb-3 w-full text-left text-sm">
					<tbody>
						{#each devices as d (d.id)}
							<tr class="border-t border-zinc-800 first:border-t-0">
								<td class="py-1.5">
									<div class="text-zinc-300">{d.name}</div>
									<div class="text-xs text-zinc-500">
										connected {formatDateTime(d.createdAt)} · last used {d.lastUsedAt
											? formatDateTime(d.lastUsedAt)
											: 'never'}{d.lastIp ? ` · ${d.lastIp}` : ''}
									</div>
								</td>
								<td class="text-right">
									<button type="button" class={secondary} onclick={() => revokeDevice(d)}
										>Disconnect</button
									>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			{/if}
			{#if newToken}
				<div class="mb-3 rounded border border-emerald-800 bg-emerald-950 p-3 text-sm">
					<p class="mb-1 text-emerald-200">
						Paste this token into the app (Settings → Server sync → "Use a device token instead").
						It is shown only once.
					</p>
					<code class="font-mono text-xs break-all text-emerald-100 select-all">{newToken}</code>
					<div class="mt-2">
						<button type="button" class={secondary} onclick={() => (newToken = null)}>Done</button>
					</div>
				</div>
			{/if}
			<details class="text-sm">
				<summary class="cursor-pointer text-zinc-400 hover:text-zinc-200">
					Create a token by hand (if the browser login doesn't work)
				</summary>
				<form class="mt-2 flex items-end gap-2" onsubmit={createToken}>
					<label class="flex flex-1 flex-col gap-1">
						<span class="text-zinc-400">Name of the computer</span>
						<input class={input} bind:value={newDeviceName} maxlength="64" />
					</label>
					<button type="submit" class={button}>Create token</button>
				</form>
			</details>
			{#if deviceMsg}<p class="mt-2 text-sm text-red-400">{deviceMsg}</p>{/if}
		</section>
	{/if}
{/if}
