<script lang="ts">
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

<details class="card-layout absolute top-2 right-3 z-10">
	<summary
		class="text-muted-foreground hover:text-foreground cursor-pointer rounded px-2 py-1 text-xs"
		aria-label={`Layout for ${title}`}>Layout</summary
	>
	<div
		class="border-border bg-popover absolute right-0 mt-1 flex w-44 flex-col gap-1 rounded-md border p-1 shadow-(--elevation-popover)"
	>
		<Button
			variant="ghost"
			size="sm"
			class="justify-start"
			disabled={first}
			onclick={() => onMove(-1)}
			aria-label={`Move ${title} up`}>Move up</Button
		>
		<Button
			variant="ghost"
			size="sm"
			class="justify-start"
			disabled={last}
			onclick={() => onMove(1)}
			aria-label={`Move ${title} down`}>Move down</Button
		>
		{#if resizable}<div class="border-border my-1 border-t"></div>
			<Button
				variant="ghost"
				size="sm"
				class="justify-start"
				onclick={() => onResize(-80)}
				aria-label={`Make ${title} shorter`}>Shorter plot</Button
			>
			<Button
				variant="ghost"
				size="sm"
				class="justify-start"
				onclick={() => onResize(80)}
				aria-label={`Make ${title} taller`}>Taller plot</Button
			>
		{/if}
	</div>
</details>
