<script lang="ts">
	import NetflowFileRouterAnalysisSection from './NetflowFileRouterAnalysisSection.svelte';
	import NetflowFileRouterSummary from './NetflowFileRouterSummary.svelte';
	import * as Card from '#lib/components/ui/card/index.ts';
	import type { NetflowFileRouterRow } from './file-detail-loader.svelte';
	import type { MaadAddressSide } from '#lib/types/types.ts';

	let {
		row,
		showSpectrum = true,
		unavailableCopy = { source: null, destination: null },
		formatCount,
		formatTimestampAsPST
	}: {
		row: NetflowFileRouterRow;
		showSpectrum?: boolean;
		unavailableCopy?: Record<MaadAddressSide, string | null>;
		formatCount: (value: number | null | undefined) => string;
		formatTimestampAsPST: (timestamp: number) => string;
	} = $props();
</script>

<Card.Root size="sm" class="grid gap-0 py-0 2xl:grid-cols-[minmax(28rem,0.8fr)_minmax(0,2fr)]">
	<div class="min-w-0 border-b 2xl:border-r 2xl:border-b-0">
		<NetflowFileRouterSummary {row} {formatCount} {formatTimestampAsPST} />
	</div>

	<Card.Content class="min-w-0 p-4">
		<h4 class="text-foreground mb-3 text-sm font-semibold">MAAD analysis</h4>
		<div class="space-y-6">
			<NetflowFileRouterAnalysisSection
				title="Structure"
				kind="structure"
				source={row.source.structure}
				destination={row.destination.structure}
				{unavailableCopy}
			/>
			{#if showSpectrum}
				<NetflowFileRouterAnalysisSection
					title="Spectrum"
					kind="spectrum"
					source={row.source.spectrum}
					destination={row.destination.spectrum}
					{unavailableCopy}
				/>
			{:else}
				<p class="text-muted-foreground text-sm">
					The spectrum is only computed for the addresses measure.
				</p>
			{/if}
		</div>
	</Card.Content>
</Card.Root>
