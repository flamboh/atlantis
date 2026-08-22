<script lang="ts">
	import NetflowFileRouterAnalysisSection from './NetflowFileRouterAnalysisSection.svelte';
	import NetflowFileRouterSummary from './NetflowFileRouterSummary.svelte';
	import * as Card from '$lib/components/ui/card';
	import type { NetflowFileRouterRow } from './file-detail-loader.svelte';

	let {
		row,
		formatCount,
		formatTimestampAsPST
	}: {
		row: NetflowFileRouterRow;
		formatCount: (value: number | null | undefined) => string;
		formatTimestampAsPST: (timestamp: number) => string;
	} = $props();
</script>

<Card.Root size="sm" class="gap-0 py-0">
	<NetflowFileRouterSummary {row} {formatCount} {formatTimestampAsPST} />

	<Card.Content class="py-4">
		<h4 class="text-md text-foreground mb-4 font-semibold">MAAD Analysis</h4>
		<div class="mb-6 grid grid-cols-1 gap-6 lg:grid-cols-2">
			<h5 class="border-border text-primary hidden border-b pb-2 text-base font-semibold lg:block">
				Source
			</h5>
			<h5 class="border-border text-primary hidden border-b pb-2 text-base font-semibold lg:block">
				Destination
			</h5>
		</div>
		<div class="space-y-6">
			<NetflowFileRouterAnalysisSection
				title="Structure"
				kind="structure"
				source={row.source.structure}
				destination={row.destination.structure}
			/>
			<NetflowFileRouterAnalysisSection
				title="Spectrum"
				kind="spectrum"
				source={row.source.spectrum}
				destination={row.destination.spectrum}
			/>
		</div>
	</Card.Content>
</Card.Root>
