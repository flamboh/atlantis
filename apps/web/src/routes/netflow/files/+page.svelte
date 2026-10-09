<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import PageHeader from '#lib/components/shell/PageHeader.svelte';
	import { Button } from '#lib/components/ui/button/index.ts';
	import * as Card from '#lib/components/ui/card/index.ts';
	import { Input } from '#lib/components/ui/input/index.ts';
	import { Label } from '#lib/components/ui/label/index.ts';
	import * as NativeSelect from '#lib/components/ui/native-select/index.ts';
	import { navigateToNetflowFile } from '#lib/utils/netflow-file-navigation.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let timestamp = $state('');
	let error = $state('');

	function selectDataset(datasetId: string) {
		void goto(`${resolve('/netflow/files')}?dataset=${encodeURIComponent(datasetId)}`, {
			replaceState: true,
			reset: false
		});
	}

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

		if (!data.selectedDataset) {
			error = 'Please choose a dataset';
			return;
		}

		void navigateToNetflowFile(goto, timestamp, data.selectedDataset);
	}
</script>

<svelte:head><title>NetFlow Files · ATLANTIS</title></svelte:head>

<div class="shell page flex flex-col gap-4">
	<PageHeader title="NetFlow Files">
		{#snippet description()}
			Open a five-minute capture to inspect its source summaries and MAAD analysis.
		{/snippet}
	</PageHeader>

	<Card.Root class="max-w-3xl gap-4">
		<Card.Header>
			<Card.Title><h2>Open a capture by timestamp</h2></Card.Title>
			<Card.Description>
				Use the 12-digit timestamp from the capture filename, e.g.
				<code class="font-mono">nfcapd.202601011200</code>.
			</Card.Description>
		</Card.Header>
		<Card.Content>
			<form
				class="grid gap-4 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] sm:items-start"
				onsubmit={(event) => {
					event.preventDefault();
					navigateToFile();
				}}
				novalidate
			>
				<div class="grid gap-1.5">
					<Label for="dataset">Dataset</Label>
					<NativeSelect.Root
						id="dataset"
						class="w-full"
						value={data.selectedDataset}
						onchange={(event) => selectDataset(event.currentTarget.value)}
					>
						{#if !data.selectedDataset}
							<NativeSelect.Option value="">Select a dataset</NativeSelect.Option>
						{/if}
						{#each data.datasets as dataset (dataset.datasetId)}
							<NativeSelect.Option value={dataset.datasetId}>{dataset.label}</NativeSelect.Option>
						{/each}
					</NativeSelect.Root>
				</div>
				<div class="grid min-w-0 gap-1.5">
					<Label for="timestamp">File Timestamp (YYYYMMDDHHmm)</Label>
					<Input
						id="timestamp"
						bind:value={timestamp}
						placeholder="202601011200"
						inputmode="numeric"
						autocomplete="off"
						class="font-mono"
						maxlength={12}
						aria-invalid={error ? true : undefined}
						aria-describedby="timestamp-error"
					/>
					<p id="timestamp-error" class="text-destructive min-h-5 text-xs" aria-live="polite">
						{error}
					</p>
				</div>
				<Button type="submit" class="sm:mt-[1.375rem]">Go to File</Button>
			</form>
		</Card.Content>
	</Card.Root>
</div>
