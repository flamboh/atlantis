<script lang="ts">
	import { MediaQuery } from 'svelte/reactivity';
	import { PanelLeftClose, PanelLeftOpen, SlidersHorizontal } from '@lucide/svelte';
	import { Button } from '#lib/components/ui/button/index.ts';
	import type { Snippet } from 'svelte';

	let {
		title,
		eyebrow,
		railLabel = 'Filters',
		rail,
		navigation,
		toolbar,
		children
	}: {
		title: string;
		eyebrow: string;
		railLabel?: string;
		rail: Snippet;
		navigation?: Snippet;
		toolbar?: Snippet;
		children: Snippet;
	} = $props();
	const id = $props.id();
	const desktop = new MediaQuery('(min-width: 1024px)', true);
	let collapsed = $state(false);
	let sheetOpen = $state(false);
	let sheetModule = $state<Promise<typeof import('./FilterSheet.svelte')> | null>(null);

	function openSheet() {
		sheetModule ??= import('./FilterSheet.svelte');
		sheetOpen = true;
	}
</script>

<svelte:window
	onresize={() => {
		if (desktop.current) sheetOpen = false;
	}}
/>

<div class="console-page">
	<div class="console-heading">
		<div class="min-w-0">
			<p class="page-eyebrow">{eyebrow}</p>
			<h1 class="page-heading break-words">{title}</h1>
		</div>
		{@render navigation?.()}
	</div>
	<div class="console-body" class:rail-collapsed={collapsed}>
		{#if desktop.current && !collapsed}
			<aside class="console-rail" aria-label={railLabel}>
				{@render rail()}
			</aside>
		{/if}
		<div class="console-workarea min-w-0">
			<div class="console-toolbar">
				{#if desktop.current}
					<Button
						variant="outline"
						size="icon-sm"
						aria-label={collapsed
							? `Show ${railLabel.toLowerCase()}`
							: `Hide ${railLabel.toLowerCase()}`}
						aria-expanded={!collapsed}
						onclick={() => (collapsed = !collapsed)}
					>
						{#if collapsed}<PanelLeftOpen size={17} />{:else}<PanelLeftClose size={17} />{/if}
					</Button>
				{:else}
					<Button
						variant="outline"
						size="sm"
						aria-label={`Open ${railLabel.toLowerCase()}`}
						aria-haspopup="dialog"
						data-filter-trigger={id}
						aria-expanded={sheetOpen}
						onclick={openSheet}><SlidersHorizontal size={15} />{railLabel}</Button
					>
				{/if}
				{@render toolbar?.()}
			</div>
			<div class="console-results">{@render children()}</div>
		</div>
	</div>
</div>

{#if !desktop.current && sheetModule}
	{#await sheetModule then { default: FilterSheet }}
		<FilterSheet
			bind:open={sheetOpen}
			title={railLabel}
			onClose={() => document.querySelector<HTMLElement>(`[data-filter-trigger="${id}"]`)?.focus()}
			>{@render rail()}</FilterSheet
		>
	{/await}
{/if}
