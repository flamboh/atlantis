<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
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
		<section
			class="dark:border-dark-border dark:bg-dark-surface rounded-lg border bg-white p-6 shadow-sm dark:shadow-none"
		>
			<h1 class="text-xl font-semibold text-gray-900 dark:text-gray-100">No datasets found</h1>
			<p class="mt-3 text-sm text-gray-600 dark:text-gray-400">
				The dashboard reads SQLite databases at
				<code class="font-mono text-gray-500 dark:text-gray-500"
					>data/&lt;dataset-id&gt;/netflow.sqlite</code
				>. Build one from your NetFlow data with the pipeline, then reload this page.
			</p>
			<p class="mt-3 text-sm text-gray-600 dark:text-gray-400">
				See <code class="font-mono text-gray-500 dark:text-gray-500">docs/user/README.md</code> in the
				repository for the setup procedure.
			</p>
		</section>
	{:else}
		<div class="grid gap-4 md:grid-cols-2">
			{#each data.datasets as dataset (dataset.datasetId)}
				<button
					type="button"
					class="dark:border-dark-border dark:bg-dark-surface dark:hover:bg-dark-subtle cursor-pointer rounded-lg border bg-white p-5 text-left shadow-sm transition hover:border-blue-400 hover:shadow dark:shadow-none dark:hover:border-blue-500"
					onclick={() => openDataset(dataset.datasetId)}
				>
					<h1 class="text-xl font-semibold text-gray-900 dark:text-gray-100">{dataset.label}</h1>
					<p class="mt-3 text-sm text-gray-600 dark:text-gray-400">
						<span class="font-mono text-gray-500 dark:text-gray-500">{dataset.datasetId}</span>
						·
						{dataset.sourceCount} source{dataset.sourceCount === 1 ? '' : 's'}
						·
						{dataset.discoveryMode}
					</p>
				</button>
			{/each}
		</div>
	{/if}
</main>
