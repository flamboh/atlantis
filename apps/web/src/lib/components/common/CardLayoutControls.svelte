<script lang="ts">
	import { ArrowUp, ArrowDown, Minus, Plus } from '@lucide/svelte';
	import { Button } from '#lib/components/ui/button/index.ts';
	let {
		title,
		first,
		last,
		onMove,
		onResize,
		resizable = true
	}: {
		title: string;
		first: boolean;
		last: boolean;
		resizable?: boolean;
		onMove: (offset: number) => void;
		onResize: (offset: number) => void;
	} = $props();
</script>

<div class="card-actions" role="group" aria-label={`Arrange ${title}`}>
	<Button
		variant="ghost"
		size="icon-sm"
		disabled={first}
		onclick={() => onMove(-1)}
		aria-label={`Move ${title} up`}
		title="Move up"><ArrowUp size={15} /></Button
	>
	<Button
		variant="ghost"
		size="icon-sm"
		disabled={last}
		onclick={() => onMove(1)}
		aria-label={`Move ${title} down`}
		title="Move down"><ArrowDown size={15} /></Button
	>
	{#if resizable}
		<Button
			variant="ghost"
			size="icon-sm"
			onclick={() => onResize(-80)}
			aria-label={`Make ${title} shorter`}
			title="Shorter plot"><Minus size={15} /></Button
		>
		<Button
			variant="ghost"
			size="icon-sm"
			onclick={() => onResize(80)}
			aria-label={`Make ${title} taller`}
			title="Taller plot"><Plus size={15} /></Button
		>
	{/if}
</div>
