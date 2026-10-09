<script lang="ts">
	import * as Command from '#lib/components/ui/command/index.ts';
	import { focusSearch } from '#lib/components/common/focus-search.ts';
	import type { DatasetSummary } from '#lib/datasets.ts';

	let {
		datasets,
		datasetId,
		onChoose
	}: {
		datasets: DatasetSummary[] | null;
		datasetId: string | null;
		onChoose: (id: string) => void;
	} = $props();
</script>

<div class="contents" {@attach focusSearch}>
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
							onSelect={() => onChoose(dataset.datasetId)}
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
</div>
