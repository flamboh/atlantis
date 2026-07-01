<script lang="ts">
	import NetflowFileRouterAnalysisSection from './NetflowFileRouterAnalysisSection.svelte';
	import NetflowFileRouterSummary from './NetflowFileRouterSummary.svelte';
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

<div class="dark:border-dark-border dark:bg-dark-surface rounded-lg border bg-white shadow-sm">
	<NetflowFileRouterSummary {row} {formatCount} {formatTimestampAsPST} />

	<div class="rounded-b-lg p-4">
		<h4 class="text-md mb-4 font-semibold text-gray-800 dark:text-gray-100">MAAD Analysis</h4>
		<div class="mb-6 grid grid-cols-1 gap-6 lg:grid-cols-2">
			<h5
				class="dark:border-dark-border hidden border-b pb-2 text-base font-semibold text-blue-700 lg:block dark:text-blue-400"
			>
				Source
			</h5>
			<h5
				class="dark:border-dark-border hidden border-b pb-2 text-base font-semibold text-blue-700 lg:block dark:text-blue-400"
			>
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
	</div>
</div>
