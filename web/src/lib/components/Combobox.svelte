<!-- SPDX-FileCopyrightText: 2026 The Stintory Contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!--
	A text field that filters a long list of options as you type.
	The value is always one of the options, or '' for none.
-->
<script lang="ts">
	import { exactOption, matchOptions } from '#lib/combobox.ts';

	let {
		value = $bindable(''),
		options,
		label,
		placeholder = '',
		class: className = '',
		onchange
	}: {
		/** The chosen option, '' for none */
		value: string;
		options: string[];
		label: string;
		/** Shown while nothing is chosen */
		placeholder?: string;
		/** Classes for the text field */
		class?: string;
		/** Called after the user picked another option or cleared it */
		onchange?: (value: string) => void;
	} = $props();

	const uid = $props.id();

	let input = $state<HTMLInputElement>();
	let list = $state<HTMLUListElement>();
	let open = $state(false);
	/** What the user typed; null while the field shows the value */
	let typed = $state<string | null>(null);
	let active = $state(-1);

	const shown = $derived(typed === null ? options : matchOptions(options, typed));

	function show() {
		if (open) return;
		open = true;
		active = shown.indexOf(value);
	}

	function choose(option: string) {
		typed = null;
		open = false;
		if (option === value) return;
		value = option;
		onchange?.(option);
	}

	/** Takes what was typed when leaving the field: a full name, nothing, or else the old value. */
	function commit() {
		if (typed === null) {
			open = false;
			return;
		}
		if (typed.trim() === '') choose('');
		else choose(exactOption(options, typed) ?? value);
	}

	function move(step: number) {
		if (!open) {
			show();
			return;
		}
		if (shown.length === 0) return;
		active = Math.min(Math.max(active + step, 0), shown.length - 1);
	}

	function onkeydown(e: KeyboardEvent) {
		switch (e.key) {
			case 'ArrowDown':
				e.preventDefault();
				move(1);
				break;
			case 'ArrowUp':
				e.preventDefault();
				move(-1);
				break;
			case 'PageDown':
				if (!open) return;
				e.preventDefault();
				move(10);
				break;
			case 'PageUp':
				if (!open) return;
				e.preventDefault();
				move(-10);
				break;
			case 'Enter':
				e.preventDefault();
				if (open && shown[active] !== undefined) choose(shown[active]);
				else commit();
				break;
			case 'Escape':
				if (!open && typed === null) return;
				e.preventDefault();
				e.stopPropagation();
				typed = null;
				open = false;
				break;
		}
	}

	$effect(() => {
		if (!open || active < 0) return;
		list?.querySelector(`#${CSS.escape(`${uid}-${active}`)}`)?.scrollIntoView({ block: 'nearest' });
	});
</script>

<div class="flex flex-col gap-1">
	<label for="{uid}-input">{label}</label>
	<div class="relative">
		<input
			bind:this={input}
			id="{uid}-input"
			type="text"
			class="{className} w-full pr-7"
			role="combobox"
			aria-expanded={open}
			aria-controls="{uid}-list"
			aria-autocomplete="list"
			aria-activedescendant={open && active >= 0 ? `${uid}-${active}` : undefined}
			autocomplete="off"
			spellcheck="false"
			{placeholder}
			value={typed ?? value}
			onfocus={(e) => e.currentTarget.select()}
			onclick={(e) => {
				// the click after focusing would put the caret in the text: typing replaces it instead
				if (!open && typed === null) e.currentTarget.select();
				show();
			}}
			oninput={(e) => {
				typed = e.currentTarget.value;
				open = true;
				active = typed.trim() ? 0 : -1;
			}}
			onblur={commit}
			onchange={(e) => e.stopPropagation()}
			{onkeydown}
		/>
		{#if value}
			<button
				type="button"
				tabindex="-1"
				class="absolute inset-y-0 right-0 flex w-7 items-center justify-center text-zinc-500 hover:text-zinc-100"
				aria-label="Clear {label.toLowerCase()}"
				onpointerdown={(e) => e.preventDefault()}
				onclick={() => choose('')}
			>
				<svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
					<path d="M1 1l8 8M9 1l-8 8" class="stroke-current" stroke-width="1.5" />
				</svg>
			</button>
		{:else}
			<button
				type="button"
				tabindex="-1"
				class="absolute inset-y-0 right-0 flex w-7 items-center justify-center text-zinc-500 hover:text-zinc-100"
				aria-label="Show {label.toLowerCase()} options"
				onpointerdown={(e) => e.preventDefault()}
				onclick={() => {
					input?.focus();
					if (open) open = false;
					else show();
				}}
			>
				<svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
					<path d="M1 3l4 4 4-4" class="fill-none stroke-current" stroke-width="1.5" />
				</svg>
			</button>
		{/if}
		{#if open}
			<ul
				bind:this={list}
				id="{uid}-list"
				role="listbox"
				aria-label={label}
				class="absolute top-full left-0 z-30 mt-1 max-h-72 w-max max-w-96 min-w-full overflow-y-auto rounded border border-zinc-700 bg-zinc-900 py-1 text-sm text-zinc-100 shadow-lg"
			>
				{#each shown as option, i (option)}
					<!-- the keys are handled in the text field, which keeps the focus -->
					<!-- svelte-ignore a11y_click_events_have_key_events -->
					<li
						id="{uid}-{i}"
						role="option"
						aria-selected={option === value}
						class="cursor-pointer px-2 py-1"
						class:bg-zinc-700={i === active}
						class:font-medium={option === value}
						onpointerdown={(e) => e.preventDefault()}
						onpointermove={() => (active = i)}
						onclick={() => choose(option)}
					>
						{option}
					</li>
				{:else}
					<li class="px-2 py-1 text-zinc-500">No matches</li>
				{/each}
			</ul>
		{/if}
	</div>
</div>
