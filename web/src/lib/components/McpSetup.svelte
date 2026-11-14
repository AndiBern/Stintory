<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!--
	How to connect an AI assistant on this computer to the app's MCP server
	(`stintory mcp`, stdio): the command for Claude Code and the entry for
	Claude Desktop's configuration.
-->
<script lang="ts">
	import type { McpCommand } from '#lib/api/index.ts';

	let { mcp }: { mcp: McpCommand } = $props();

	const quote = (s: string) => (/^[\w./:\\-]+$/.test(s) ? s : `"${s.replaceAll('"', '\\"')}"`);
	const claudeCode = $derived(
		[
			'claude mcp add stintory',
			...Object.entries(mcp.env).map(([k, v]) => `-e ${quote(`${k}=${v}`)}`),
			'--',
			quote(mcp.command),
			...mcp.args.map(quote)
		].join(' ')
	);
	const claudeDesktop = $derived(
		JSON.stringify(
			{
				mcpServers: {
					stintory: {
						command: mcp.command,
						args: mcp.args,
						...(Object.keys(mcp.env).length > 0 ? { env: mcp.env } : {})
					}
				}
			},
			null,
			2
		)
	);

	let copied = $state<string | null>(null);
	async function copy(what: string, text: string) {
		try {
			await navigator.clipboard.writeText(text);
			copied = what;
			setTimeout(() => (copied = null), 2000);
		} catch {
			// select the text by hand
		}
	}

	const button =
		'rounded border border-zinc-700 px-2 py-0.5 text-xs text-zinc-300 hover:bg-zinc-800';
</script>

<section class="mb-6 max-w-2xl rounded border border-zinc-800 p-4">
	<h2 class="mb-1 font-medium">AI assistants</h2>
	<p class="mb-3 text-sm text-zinc-400">
		An AI assistant on this computer can read your sessions, laps and the corner analysis and tell
		you where you lose time. It starts Stintory in the background over MCP (Model Context Protocol);
		no server is needed. It can only read: it can't change, delete or share anything.
	</p>

	<div class="mb-1 flex items-center justify-between text-sm">
		<span class="text-zinc-300">Claude Code</span>
		<button type="button" class={button} onclick={() => copy('code', claudeCode)}
			>{copied === 'code' ? 'Copied' : 'Copy'}</button
		>
	</div>
	<pre
		class="mb-3 overflow-x-auto rounded bg-zinc-950 p-2 font-mono text-xs whitespace-pre-wrap text-zinc-200 select-all">{claudeCode}</pre>

	<div class="mb-1 flex items-center justify-between text-sm">
		<span class="text-zinc-300">
			Claude Desktop <span class="text-zinc-500">(Settings → Developer → Edit Config)</span>
		</span>
		<button type="button" class={button} onclick={() => copy('desktop', claudeDesktop)}
			>{copied === 'desktop' ? 'Copied' : 'Copy'}</button
		>
	</div>
	<pre
		class="mb-3 overflow-x-auto rounded bg-zinc-950 p-2 font-mono text-xs text-zinc-200 select-all">{claudeDesktop}</pre>

	<p class="text-xs text-zinc-500">
		Other assistants: start <code>{quote(mcp.command)} {mcp.args.join(' ')}</code> as a local (stdio)
		MCP server. It reads the laps on this computer; laps others share with you on a server are in the
		server's MCP endpoint.
	</p>
</section>
