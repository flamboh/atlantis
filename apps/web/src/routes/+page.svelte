<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { Button } from '#lib/components/ui/button/index.ts';
	import AnalysisLayout from '#lib/components/common/AnalysisLayout.svelte';
	import { ArrowUpRight } from '@lucide/svelte';
	import { Card, CardContent, CardHeader, CardTitle } from '#lib/components/ui/card/index.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	function openDataset(datasetId: string) {
		goto(resolve('/datasets/[dataset]', { dataset: datasetId }));
	}
</script>

<svelte:head>
	<title>ATLANTIS Datasets</title>
	<meta name="description" content="Select an ATLANTIS dataset dashboard" />
</svelte:head>

<AnalysisLayout
	title="Datasets"
	eyebrow="ATLANTIS / Network analysis"
	railLabel="Dataset directory"
>
	{#snippet toolbar()}<p class="text-muted-foreground text-sm">
			{data.datasets.length}
			{data.datasets.length === 1 ? 'dataset' : 'datasets'} available
		</p>{/snippet}
	{#snippet rail()}<h2 class="rail-heading">Network analysis</h2>
		<p class="text-muted-foreground text-sm leading-6">
			Select a dataset to explore its traffic and analysis.
		</p>
		<div class="rail-section">
			<h2 class="rail-heading">Dashboard</h2>
			<p class="text-muted-foreground text-xs leading-5">
				Traffic, flow characteristics, address counts, MAAD and source coverage.
			</p>
		</div>
		<div class="rail-section">
			<h2 class="rail-heading">Files</h2>
			<p class="text-muted-foreground text-xs leading-5">
				Open a five-minute capture by its filename timestamp.
			</p>
		</div>{/snippet}

	{#if data.datasets.length === 0}
		<Card class="gap-0 rounded-lg border py-6 shadow-sm ring-0">
			<CardHeader class="px-6">
				<CardTitle><h1 class="text-xl font-semibold">No datasets found</h1></CardTitle>
			</CardHeader>
			<CardContent class="px-6">
				<p class="text-muted-foreground text-sm">
					The dashboard reads SQLite databases at
					<code class="font-mono">data/&lt;dataset-id&gt;/netflow.sqlite</code>. Build one from your
					NetFlow data with the pipeline, then reload this page.
				</p>
				<p class="text-muted-foreground text-sm">
					See <code class="font-mono">docs/user/README.md</code> in the repository for the setup procedure.
				</p>
			</CardContent>
		</Card>
	{:else}<div class="catalog">
			<div class="catalog-row catalog-header" aria-hidden="true">
				<span>Dataset</span><span>Discovery</span><span>Open</span>
			</div>
			{#each data.datasets as dataset (dataset.datasetId)}
				<Button
					variant="ghost"
					class="catalog-row h-auto w-full rounded-none text-left whitespace-normal"
					onclick={() => openDataset(dataset.datasetId)}
				>
					<span class="min-w-0"
						><span class="block font-semibold break-words">{dataset.label}</span><span
							class="text-muted-foreground mt-1 block font-mono text-xs break-all"
							>{dataset.datasetId}</span
						></span
					>
					<span class="catalog-mode text-muted-foreground text-xs">{dataset.discoveryMode}</span
					><ArrowUpRight size={16} aria-hidden="true" />
				</Button>
			{/each}
		</div>
	{/if}
</AnalysisLayout>
