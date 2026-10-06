<script lang="ts">
	import { goto } from '$app/navigation';
	import AnalysisLayout from '#lib/components/common/AnalysisLayout.svelte';
	import { Button } from '#lib/components/ui/button/index.ts';
	import { Card, CardContent, CardHeader, CardTitle } from '#lib/components/ui/card/index.ts';
	import { navigateToNetflowFile } from '#lib/utils/netflow-file-navigation.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let timestamp = $state('');
	let selectedDatasetOverride = $state<string | null>(null);
	const selectedDataset = $derived(selectedDatasetOverride ?? data.selectedDataset);
	let error = $state('');

	function navigateToFile() {
		error = '';

		if (!timestamp) {
			error = 'Please enter a timestamp';
			return;
		}

		if (timestamp.length !== 12 || !/^\d{12}$/.test(timestamp)) {
			error = 'Invalid format. Expected 12 digits (YYYYMMDDHHmm)';
			return;
		}

		if (!selectedDataset) {
			error = 'Please choose a dataset';
			return;
		}

		void navigateToNetflowFile(goto, timestamp, selectedDataset);
	}

	function handleKeydown(event: KeyboardEvent) {
		if (event.key === 'Enter') {
			navigateToFile();
		}
	}
</script>

<AnalysisLayout title="NetFlow Files" eyebrow="Capture lookup" railLabel="Lookup guide">
	{#snippet toolbar()}<p class="text-muted-foreground text-sm">
			Open a five-minute capture
		</p>{/snippet}
	{#snippet rail()}<h2 class="rail-heading">File lookup</h2>
		<ol class="text-muted-foreground space-y-4 text-sm leading-6">
			<li>1. Choose a dataset.</li>
			<li>2. Enter the timestamp from the capture filename.</li>
			<li>3. Inspect source summaries and MAAD analysis.</li>
		</ol>
		<p class="text-muted-foreground mt-6 text-xs leading-5">
			The timestamp has twelve digits, in YYYYMMDDHHmm order.
		</p>{/snippet}

	<Card class="border-border bg-card mb-6 gap-3 rounded-none border py-4 ring-0">
		<CardHeader class="px-4">
			<CardTitle><h2 class="text-lg font-semibold">Navigate to File by Timestamp</h2></CardTitle>
		</CardHeader>
		<CardContent class="px-4">
			<div class="grid max-w-3xl gap-4 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
				<div>
					<label for="dataset" class="text-foreground mb-1 block text-sm font-medium">Dataset</label
					>
					<select
						id="dataset"
						value={selectedDataset}
						onchange={(event) => {
							selectedDatasetOverride = event.currentTarget.value;
						}}
						class="border-input bg-background text-foreground focus-visible:ring-ring w-full rounded border px-3 py-2 focus-visible:ring-2 focus-visible:outline-none"
					>
						{#if !selectedDataset}
							<option value="">Select a dataset</option>
						{/if}
						{#each data.datasets as dataset (dataset.datasetId)}
							<option value={dataset.datasetId}>{dataset.label}</option>
						{/each}
					</select>
				</div>
				<div class="min-w-0">
					<label for="timestamp" class="text-foreground mb-1 block text-sm font-medium">
						File Timestamp (YYYYMMDDHHmm)
					</label>
					<input
						id="timestamp"
						type="text"
						bind:value={timestamp}
						onkeydown={handleKeydown}
						placeholder="202601011200"
						class="border-input bg-background text-foreground placeholder:text-muted-foreground focus-visible:ring-ring w-full rounded border px-3 py-2 focus-visible:ring-2 focus-visible:outline-none"
						maxlength="12"
					/>
					<div
						class={`mt-1 min-h-6 text-sm ${error ? 'text-destructive' : 'text-transparent'}`}
						aria-live="polite"
					>
						{error || ' '}
					</div>
				</div>
				<div class="flex items-start sm:col-span-2">
					<Button onclick={navigateToFile} class="w-full px-4 sm:w-auto">Go to File</Button>
				</div>
			</div>
			<p class="text-muted-foreground mt-2 text-sm">
				Choose a dataset, then enter the exact 12-digit timestamp from NetFlow filenames (e.g.,
				`nfcapd.202601011200`).
			</p>
		</CardContent>
	</Card>
</AnalysisLayout>
