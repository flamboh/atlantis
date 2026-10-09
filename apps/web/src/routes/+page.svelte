<script lang="ts">
	import { resolve } from '$app/paths';
	import { ArrowRight, Database } from '@lucide/svelte';
	import PageHeader from '#lib/components/shell/PageHeader.svelte';
	import { Badge } from '#lib/components/ui/badge/index.ts';
	import * as Card from '#lib/components/ui/card/index.ts';
	import * as Empty from '#lib/components/ui/empty/index.ts';
	import * as Table from '#lib/components/ui/table/index.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<svelte:head>
	<title>ATLANTIS Datasets</title>
	<meta name="description" content="Select an ATLANTIS dataset dashboard" />
</svelte:head>

<div class="shell page flex flex-col gap-4">
	<PageHeader title="Datasets">
		{#snippet description()}
			Choose a dataset to explore its traffic, flow characteristics, address counts, MAAD and
			coverage.
		{/snippet}
	</PageHeader>

	{#if data.datasets.length === 0}
		<Card.Root class="py-0">
			<Empty.Root class="py-12">
				<Empty.Header>
					<Empty.Media variant="icon"><Database /></Empty.Media>
					<Empty.Title><h2>No datasets found</h2></Empty.Title>
					<Empty.Description>
						The dashboard reads SQLite databases at
						<code class="font-mono">data/&lt;dataset-id&gt;/netflow.sqlite</code>. Build one from
						your NetFlow data with the pipeline, then reload this page. See
						<code class="font-mono">docs/user/README.md</code> for the setup procedure.
					</Empty.Description>
				</Empty.Header>
			</Empty.Root>
		</Card.Root>
	{:else}
		<Card.Root class="overflow-hidden py-0">
			<Table.Root>
				<Table.Header class="bg-muted/50">
					<Table.Row class="hover:bg-transparent">
						<Table.Head class="text-muted-foreground h-9 px-4 text-xs">Dataset</Table.Head>
						<Table.Head class="text-muted-foreground hidden h-9 px-4 text-xs sm:table-cell"
							>Data from</Table.Head
						>
						<Table.Head class="text-muted-foreground h-9 px-4 text-xs">Discovery</Table.Head>
						<Table.Head class="h-9 w-10 px-4"><span class="sr-only">Open</span></Table.Head>
					</Table.Row>
				</Table.Header>
				<Table.Body>
					{#each data.datasets as dataset (dataset.datasetId)}
						<Table.Row class="group relative">
							<Table.Cell class="px-4 py-3">
								<a
									href={resolve('/datasets/[dataset]', { dataset: dataset.datasetId })}
									class="font-medium after:absolute after:inset-0 after:content-['']"
									>{dataset.label}</a
								>
								<span class="text-muted-foreground block font-mono text-xs break-all"
									>{dataset.datasetId}</span
								>
							</Table.Cell>
							<Table.Cell class="text-muted-foreground hidden px-4 py-3 tabular-nums sm:table-cell"
								>{dataset.defaultStartDate}</Table.Cell
							>
							<Table.Cell class="px-4 py-3">
								<Badge variant="outline" class="text-muted-foreground font-normal"
									>{dataset.discoveryMode}</Badge
								>
								{#if dataset.isDefault}<Badge
										class="bg-selection text-selection-foreground ml-1 font-normal">default</Badge
									>{/if}
							</Table.Cell>
							<Table.Cell class="text-muted-foreground group-hover:text-foreground px-4 py-3">
								<ArrowRight class="size-4" aria-hidden="true" />
							</Table.Cell>
						</Table.Row>
					{/each}
				</Table.Body>
			</Table.Root>
		</Card.Root>
	{/if}
</div>
