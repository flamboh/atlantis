<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { Button } from '$lib/components/ui/button';
	import { Card, CardContent, CardHeader, CardTitle } from '$lib/components/ui/card';
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

<main class="mx-auto flex max-w-[95vw] flex-col gap-4 px-4 py-8 sm:px-2 lg:px-4">
	{#if data.datasets.length === 0}
		<Card class="gap-0 rounded-lg border py-6 shadow-sm ring-0">
			<CardHeader class="px-6">
				<CardTitle><h1 class="text-xl font-semibold">No datasets found</h1></CardTitle>
			</CardHeader>
			<CardContent class="px-6">
				<p class="text-muted-foreground mt-3 text-sm">
					The dashboard reads SQLite databases at
					<code class="font-mono">data/&lt;dataset-id&gt;/netflow.sqlite</code>. Build one from your
					NetFlow data with the pipeline, then reload this page.
				</p>
				<p class="text-muted-foreground mt-3 text-sm">
					See <code class="font-mono">docs/user/README.md</code> in the repository for the setup procedure.
				</p>
			</CardContent>
		</Card>
	{:else}
		<div class="grid gap-4 md:grid-cols-2">
			{#each data.datasets as dataset (dataset.datasetId)}
				<Card
					class="hover:border-primary gap-0 rounded-lg border py-0 shadow-sm ring-0 transition hover:shadow"
				>
					<Button
						variant="ghost"
						class="h-auto w-full cursor-pointer justify-start rounded-lg p-5 text-left whitespace-normal"
						onclick={() => openDataset(dataset.datasetId)}
					>
						<div>
							<h1 class="text-foreground text-xl font-semibold">{dataset.label}</h1>
							<p class="text-muted-foreground mt-3 text-sm">
								<span class="font-mono">{dataset.datasetId}</span>
								·
								{dataset.discoveryMode}
							</p>
						</div>
					</Button>
				</Card>
			{/each}
		</div>
	{/if}
</main>
