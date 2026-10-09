<script lang="ts">
	import { goto } from '$app/navigation';
	import { ChevronsUpDown } from '@lucide/svelte';
	import * as Popover from '#lib/components/ui/popover/index.ts';
	import { Button } from '#lib/components/ui/button/index.ts';
	import type { DatasetSummary } from '#lib/datasets.ts';
	import { sectionHref, type AppSection } from './app-context.ts';

	let {
		datasets,
		datasetId,
		section,
		onRequestDatasets
	}: {
		datasets: DatasetSummary[] | null;
		datasetId: string | null;
		section: AppSection;
		onRequestDatasets: () => void;
	} = $props();

	let open = $state(false);
	let list = $state<Promise<typeof import('./DatasetList.svelte')> | null>(null);
	const current = $derived(datasets?.find((dataset) => dataset.datasetId === datasetId));
	const label = $derived(current?.label ?? datasetId ?? 'Select dataset');

	function choose(id: string) {
		open = false;
		const target = section === 'datasets' ? 'dashboard' : section;
		void goto(sectionHref(target, id));
	}
</script>

<Popover.Root
	bind:open
	onOpenChange={(next) => {
		if (!next) return;
		list ??= import('./DatasetList.svelte');
		if (!datasets) onRequestDatasets();
	}}
>
	<Popover.Trigger>
		{#snippet child({ props })}
			<Button
				{...props}
				variant="ghost"
				size="sm"
				class="max-w-[min(18rem,calc(100vw-13.5rem))] gap-1.5 px-2 font-medium"
				aria-label={`Dataset: ${label}`}
			>
				<span class="truncate">{label}</span>
				<ChevronsUpDown class="text-muted-foreground size-3.5" />
			</Button>
		{/snippet}
	</Popover.Trigger>
	<Popover.Content align="start" class="w-72 gap-0 p-0">
		{#if list}
			{#await list then { default: DatasetList }}
				<DatasetList {datasets} {datasetId} onChoose={choose} />
			{/await}
		{/if}
	</Popover.Content>
</Popover.Root>
