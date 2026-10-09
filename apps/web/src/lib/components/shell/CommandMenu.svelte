<script lang="ts">
	import { Search } from '@lucide/svelte';
	import { Button } from '#lib/components/ui/button/index.ts';
	import { Kbd } from '#lib/components/ui/kbd/index.ts';
	import type { DatasetSummary } from '#lib/datasets.ts';

	let {
		datasets,
		datasetId,
		onRequestDatasets
	}: {
		datasets: DatasetSummary[] | null;
		datasetId: string | null;
		onRequestDatasets: () => void;
	} = $props();

	let open = $state(false);
	let palette = $state<Promise<typeof import('./CommandPalette.svelte')> | null>(null);

	function setOpen(next: boolean) {
		open = next;
		if (!next) return;
		palette ??= import('./CommandPalette.svelte');
		if (!datasets) onRequestDatasets();
	}
</script>

<svelte:window
	onkeydown={(event) => {
		if (event.key.toLowerCase() === 'k' && (event.metaKey || event.ctrlKey) && !event.altKey) {
			event.preventDefault();
			setOpen(!open);
		}
	}}
/>

<Button
	variant="ghost"
	size="sm"
	class="text-muted-foreground gap-2 px-2"
	aria-label="Open command menu"
	aria-keyshortcuts="Control+K Meta+K"
	onclick={() => setOpen(true)}
>
	<Search class="size-4" />
	<span class="hidden md:inline">Search</span>
	<Kbd class="hidden md:inline-flex">⌘K</Kbd>
</Button>

{#if palette}
	{#await palette then { default: CommandPalette }}
		<CommandPalette bind:open {datasets} {datasetId} />
	{/await}
{/if}
