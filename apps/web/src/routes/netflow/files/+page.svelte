<script lang="ts">
	import { goto } from '$app/navigation';
	import { Button } from '$lib/components/ui/button';
	import { Card, CardContent, CardHeader, CardTitle } from '$lib/components/ui/card';
	import { navigateToNetflowFile } from '$lib/utils/netflow-file-navigation';
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

<div class="mx-auto max-w-[95vw] px-4 py-8 sm:px-2 lg:px-4">
	<h1 class="text-foreground mb-4 text-2xl">NetFlow Files</h1>

	<Card class="border-primary/20 bg-primary/5 mb-6 gap-3 rounded-lg border py-4 ring-0">
		<CardHeader class="px-4">
			<CardTitle><h2 class="text-lg font-semibold">Navigate to File by Timestamp</h2></CardTitle>
		</CardHeader>
		<CardContent class="px-4">
			<div class="grid gap-3 lg:grid-cols-[14rem_minmax(0,1fr)_auto]">
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
				<div class="flex items-start lg:pt-6">
					<Button onclick={navigateToFile} class="w-full px-4 lg:w-auto">Go to File</Button>
				</div>
			</div>
			<p class="text-muted-foreground mt-2 text-sm">
				Choose a dataset, then enter the exact 12-digit timestamp from NetFlow filenames (e.g.,
				`nfcapd.202601011200`).
			</p>
		</CardContent>
	</Card>
</div>
