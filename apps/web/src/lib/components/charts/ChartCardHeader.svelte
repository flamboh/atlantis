<script lang="ts">
	import { GripVertical } from '@lucide/svelte';
	import type { Snippet } from 'svelte';
	import CardMenu from './CardMenu.svelte';
	import { optionalCardFrame } from './card-frame.ts';

	let {
		title,
		subtitle = null,
		controls
	}: { title: string; subtitle?: string | null; controls?: Snippet } = $props();

	const frame = optionalCardFrame();
</script>

<div
	class="chart-card-header border-border flex min-h-12 flex-wrap items-center gap-x-3 gap-y-2 border-b px-3 py-2"
>
	<div
		class={[
			'flex min-w-0 flex-1 items-center gap-1 self-stretch',
			frame && 'cursor-grab select-none active:cursor-grabbing'
		]}
		draggable={frame ? 'true' : undefined}
		data-drag-handle={frame ? '' : undefined}
	>
		{#if frame}<GripVertical
				class="text-muted-foreground/50 -ml-1 size-4 shrink-0"
				aria-hidden="true"
			/>{/if}
		<h2 class="text-foreground truncate text-sm font-semibold tracking-tight">{title}</h2>
		{#if subtitle}<span class="text-muted-foreground ml-1.5 truncate text-xs">{subtitle}</span>{/if}
	</div>
	{#if controls || frame}
		<div class="ml-auto flex flex-wrap items-center justify-end gap-1.5">
			{@render controls?.()}
			{#if frame}<CardMenu {frame} />{/if}
		</div>
	{/if}
</div>
