<script lang="ts">
	import NetflowFileAnalysisPane from './NetflowFileAnalysisPane.svelte';
	import type { FileDetailResourceView } from './file-detail-loader.svelte';
	import type { SpectrumData, StructureFunctionData } from '$lib/types/types';

	type AnalysisKind = 'structure' | 'spectrum';

	let {
		title,
		kind,
		source,
		destination
	}: {
		title: string;
		kind: AnalysisKind;
		source: FileDetailResourceView<StructureFunctionData | SpectrumData>;
		destination: FileDetailResourceView<StructureFunctionData | SpectrumData>;
	} = $props();
</script>

<div class="space-y-3">
	<h6 class="text-md text-foreground font-medium">{title}</h6>
	<div class="grid grid-cols-1 gap-6 lg:grid-cols-2">
		<div>
			<NetflowFileAnalysisPane {kind} sideLabel="source" slot={source} />
		</div>
		<div>
			<NetflowFileAnalysisPane {kind} sideLabel="destination" slot={destination} />
		</div>
	</div>
</div>
