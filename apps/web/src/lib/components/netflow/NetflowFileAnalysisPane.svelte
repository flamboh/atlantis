<script lang="ts">
	import SpectrumChart from '$lib/components/charts/SpectrumChart.svelte';
	import StructureFunctionChart from '$lib/components/charts/StructureFunctionChart.svelte';
	import { Button } from '$lib/components/ui/button';
	import type { FileDetailResourceView } from './file-detail-loader.svelte';
	import type { SpectrumData, StructureFunctionData } from '$lib/types/types';

	type AnalysisKind = 'structure' | 'spectrum';

	let {
		kind,
		sideLabel,
		slot
	}: {
		kind: AnalysisKind;
		sideLabel: 'source' | 'destination';
		slot: FileDetailResourceView<StructureFunctionData | SpectrumData>;
	} = $props();

	const kindLabel = $derived(kind === 'structure' ? 'structure' : 'spectrum');
	const loadingLabel = $derived(`Loading ${sideLabel} ${kindLabel}...`);
	const emptyLabel = $derived(`No ${sideLabel} ${kindLabel} data.`);
	const errorLabel = $derived(`Error loading ${sideLabel} ${kindLabel}:`);
</script>

{#if slot.loading && slot.data === null}
	<div class="flex items-center justify-center py-6">
		<div class="text-muted-foreground">{loadingLabel}</div>
	</div>
{:else if slot.error && slot.data === null}
	<div class="border-destructive/20 bg-destructive/5 text-destructive rounded border p-4">
		<p>{errorLabel} {slot.error}</p>
		<Button variant="destructive" size="sm" class="mt-2" onclick={slot.refresh}>Retry</Button>
	</div>
{:else if slot.data}
	<div class="space-y-3">
		{#if slot.loading}
			<div class="text-muted-foreground text-sm">
				Refreshing {sideLabel}
				{kindLabel}...
			</div>
		{/if}
		{#if slot.error}
			<div
				class="border-destructive/20 bg-destructive/5 text-destructive rounded border p-3 text-sm"
			>
				<p>{errorLabel} {slot.error}</p>
				<Button variant="destructive" size="sm" class="mt-2" onclick={slot.refresh}>Retry</Button>
			</div>
		{/if}
		{#if kind === 'structure'}
			<StructureFunctionChart data={slot.data as StructureFunctionData} />
		{:else}
			<SpectrumChart data={slot.data as SpectrumData} />
		{/if}
	</div>
{:else}
	<div class="space-y-3">
		<div class="text-muted-foreground text-sm">{emptyLabel}</div>
		<Button size="sm" onclick={slot.refresh}>Reload</Button>
	</div>
{/if}
