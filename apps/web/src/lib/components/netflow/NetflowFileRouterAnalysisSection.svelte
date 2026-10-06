<script lang="ts">
	import NetflowFileAnalysisPane from './NetflowFileAnalysisPane.svelte';
	import type { FileDetailResourceView } from './file-detail-loader.svelte';
	import type { MaadAddressSide, SpectrumData, StructureFunctionData } from '#lib/types/types.ts';

	type AnalysisKind = 'structure' | 'spectrum';

	let {
		title,
		kind,
		source,
		destination,
		unavailableCopy
	}: {
		title: string;
		kind: AnalysisKind;
		source: FileDetailResourceView<StructureFunctionData | SpectrumData>;
		destination: FileDetailResourceView<StructureFunctionData | SpectrumData>;
		unavailableCopy: Record<MaadAddressSide, string | null>;
	} = $props();
</script>

<div class="space-y-3">
	<h6 class="text-md text-foreground font-medium">{title}</h6>
	<div class="grid grid-cols-1 gap-6 lg:grid-cols-2">
		<div class="min-w-0">
			<h5 class="text-muted-foreground mb-2 text-sm font-medium">Source · {title}</h5>
			<NetflowFileAnalysisPane
				{kind}
				sideLabel="source"
				slot={source}
				unavailableCopy={unavailableCopy.source}
			/>
		</div>
		<div class="min-w-0">
			<h5 class="text-muted-foreground mb-2 text-sm font-medium">Destination · {title}</h5>
			<NetflowFileAnalysisPane
				{kind}
				sideLabel="destination"
				slot={destination}
				unavailableCopy={unavailableCopy.destination}
			/>
		</div>
	</div>
</div>
