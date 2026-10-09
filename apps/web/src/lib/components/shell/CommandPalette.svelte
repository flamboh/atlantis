<script lang="ts">
	import { goto } from '$app/navigation';
	import * as Command from '#lib/components/ui/command/index.ts';
	import type { DatasetSummary } from '#lib/datasets.ts';
	import { theme } from '#lib/stores/theme.svelte.ts';
	import { sectionHref, sectionsFor, type AppSection } from './app-context.ts';

	let {
		open = $bindable(false),
		datasets,
		datasetId
	}: { open?: boolean; datasets: DatasetSummary[] | null; datasetId: string | null } = $props();

	function run(action: () => void) {
		open = false;
		action();
	}

	function go(section: AppSection, id: string | null) {
		run(() => void goto(sectionHref(section, id)));
	}
</script>

<Command.Dialog
	bind:open
	title="Command menu"
	description="Jump to a page or dataset, or switch the theme."
>
	<Command.Input placeholder="Type a command or search…" aria-label="Search commands" />
	<Command.List>
		<Command.Empty>No results.</Command.Empty>
		<Command.Group heading="Pages">
			{#each sectionsFor(datasetId) as item (item.section)}
				<Command.Item value={`page ${item.label}`} onSelect={() => go(item.section, datasetId)}
					>{item.label}</Command.Item
				>
			{/each}
		</Command.Group>
		{#if datasets && datasets.length > 0}
			<Command.Group heading="Datasets">
				{#each datasets as dataset (dataset.datasetId)}
					<Command.Item
						value={`dataset ${dataset.label} ${dataset.datasetId}`}
						data-checked={dataset.datasetId === datasetId}
						onSelect={() => go('dashboard', dataset.datasetId)}
						>{dataset.label}
						<span class="text-muted-foreground font-mono text-xs">{dataset.datasetId}</span
						></Command.Item
					>
				{/each}
			</Command.Group>
		{/if}
		<Command.Group heading="Preferences">
			<Command.Item
				value={`Switch to ${theme.dark ? 'light' : 'dark'} theme appearance`}
				onSelect={() => run(() => theme.toggle())}
				>Switch to {theme.dark ? 'light' : 'dark'} theme</Command.Item
			>
		</Command.Group>
	</Command.List>
</Command.Dialog>
