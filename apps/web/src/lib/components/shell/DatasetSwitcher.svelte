<script lang="ts">
	import { goto } from '$app/navigation';
	import { ChevronsUpDown } from '@lucide/svelte';
	import * as Command from '#lib/components/ui/command/index.ts';
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
		if (next && !datasets) onRequestDatasets();
	}}
>
	<Popover.Trigger>
		{#snippet child({ props })}
			<Button
				{...props}
				variant="ghost"
				size="sm"
				class="max-w-[min(18rem,45vw)] gap-1.5 px-2 font-medium"
				aria-label={`Dataset: ${label}`}
			>
				<span class="truncate">{label}</span>
				<ChevronsUpDown class="text-muted-foreground size-3.5" />
			</Button>
		{/snippet}
	</Popover.Trigger>
	<Popover.Content align="start" class="w-72 gap-0 p-0">
		<Command.Root>
			<Command.Input placeholder="Search datasets…" aria-label="Search datasets" />
			<Command.List class="max-h-72">
				<Command.Empty>{datasets ? 'No datasets found.' : 'Loading datasets…'}</Command.Empty>
				{#if datasets}
					<Command.Group heading="Datasets">
						{#each datasets as dataset (dataset.datasetId)}
							<Command.Item
								value={`${dataset.label} ${dataset.datasetId}`}
								data-checked={dataset.datasetId === datasetId}
								onSelect={() => choose(dataset.datasetId)}
							>
								<span class="flex min-w-0 flex-col">
									<span class="truncate">{dataset.label}</span>
									<span class="text-muted-foreground truncate font-mono text-xs"
										>{dataset.datasetId}</span
									>
								</span>
							</Command.Item>
						{/each}
					</Command.Group>
				{/if}
			</Command.List>
		</Command.Root>
	</Popover.Content>
</Popover.Root>
