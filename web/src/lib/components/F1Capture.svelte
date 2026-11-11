<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<script lang="ts">
	import { untrack } from 'svelte';
	import { api, type F1CaptureSettings, type Settings } from '#lib/api/index.ts';
	import { errorText } from '#lib/app-state.svelte.ts';

	let { settings, onsaved }: { settings: Settings; onsaved: () => void } = $props();

	const client = api();
	// the form starts from the saved settings; the page re-creates it after saving
	let form = $state<F1CaptureSettings>(untrack(() => $state.snapshot(settings.f1Capture)));
	let forward = $state(untrack(() => settings.f1Capture.forward.join('\n')));
	let error = $state<string | null>(null);
	let saved = $state(false);
	let busy = $state(false);

	async function save(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = null;
		saved = false;
		try {
			await client.setF1Capture({
				...form,
				port: Number(form.port),
				forward: forward
					.split('\n')
					.map((s) => s.trim())
					.filter(Boolean)
			});
			saved = true;
			onsaved();
		} catch (e) {
			error = errorText(e);
		} finally {
			busy = false;
		}
	}
</script>

<section class="mb-6 max-w-2xl rounded border border-zinc-800 p-4">
	<h2 class="mb-1 font-medium">F1 25</h2>
	<p class="mb-3 text-sm text-zinc-400">
		The game sends its telemetry over the network (UDP). In F1 25 open
		<em>Settings → Telemetry Settings</em> and set: UDP Telemetry <strong>On</strong>, Broadcast
		Mode <strong>Off</strong>, IP address <strong>127.0.0.1</strong> (on this PC) or this PC's
		network address (console, other PC), port <strong>{form.port}</strong>, send rate
		<strong>60 Hz</strong>, UDP format <strong>2026</strong> or <strong>2025</strong>.
	</p>
	<p class="mb-3 text-sm" role="status">
		{#if settings.f1Error}
			<span class="text-amber-400">{settings.f1Error}</span>
		{:else if settings.f1Listening}
			<span class="text-zinc-300">Listening on {settings.f1Listening}.</span>
		{:else if settings.f1Capture.enabled}
			<span class="text-zinc-400">Starting…</span>
		{:else}
			<span class="text-zinc-400">Off.</span>
		{/if}
	</p>
	<form class="flex flex-col gap-3 text-sm" onsubmit={save}>
		<label class="flex items-center gap-2">
			<input type="checkbox" bind:checked={form.enabled} />
			Record F1 25
		</label>
		<label class="flex items-center gap-2">
			<span class="w-40 text-zinc-400">UDP port</span>
			<input
				type="number"
				min="1024"
				max="65535"
				class="w-28 rounded border border-zinc-700 bg-zinc-950 px-2 py-1"
				bind:value={form.port}
			/>
		</label>
		<label class="flex items-start gap-2">
			<input type="checkbox" bind:checked={form.allowRemote} class="mt-1" />
			<span>
				Accept data from other devices
				<span class="block text-xs text-zinc-400"
					>For a PlayStation, Xbox or another PC on your network. Your firewall may ask to allow
					Stintory.</span
				>
			</span>
		</label>
		<label class="flex flex-col gap-1">
			<span class="text-zinc-400"
				>Forward to other apps (one <code>address:port</code> per line)</span
			>
			<textarea
				rows="2"
				class="rounded border border-zinc-700 bg-zinc-950 px-2 py-1 font-mono text-xs"
				placeholder="127.0.0.1:20778"
				bind:value={forward}></textarea>
			<span class="text-xs text-zinc-500"
				>Every packet is passed on unchanged, so other telemetry apps keep working. Set them to
				listen on that port.</span
			>
		</label>
		<div class="flex items-center gap-3">
			<button
				type="submit"
				class="rounded bg-zinc-100 px-3 py-1.5 font-medium text-zinc-900 hover:bg-white disabled:opacity-50"
				disabled={busy}>Save</button
			>
			{#if error}<span class="text-red-400" role="alert">{error}</span>{/if}
			{#if saved}<span class="text-zinc-300" role="status"
					>Saved. Applies once no F1 session is being recorded.</span
				>{/if}
		</div>
	</form>
</section>
