<script lang="ts">
	import ChartPlot from './ChartPlot.svelte';
	import type { StructureFunctionData } from '#lib/types/types.ts';
	import type { PlotAnnotation } from './chart-registry';
	let { data }: { data: StructureFunctionData } = $props();
	const points = $derived(
		data.structureFunction.filter((point) => Number.isFinite(point.q) && Number.isFinite(point.tau))
	);
	const series = $derived([
		{
			label: 'tau(q)',
			color: 'rgb(59,130,246)',
			strokeWidth: 2,
			data: points.map((point) => ({ x: point.q, y: point.tau, sd: point.sd }))
		}
	]);
	const annotations = $derived<PlotAnnotation[]>(
		points
			.filter((point) => Number.isFinite(point.sd))
			.flatMap((point) => [
				{
					color: 'rgb(128,128,128)',
					opacity: 0.7,
					data: [
						{ x: point.q, y: point.tau - point.sd },
						{ x: point.q, y: point.tau + point.sd }
					]
				},
				{
					color: 'rgb(128,128,128)',
					opacity: 0.7,
					data: [
						{ x: point.q - 0.02, y: point.tau - point.sd },
						{ x: point.q + 0.02, y: point.tau - point.sd }
					]
				},
				{
					color: 'rgb(128,128,128)',
					opacity: 0.7,
					data: [
						{ x: point.q - 0.02, y: point.tau + point.sd },
						{ x: point.q + 0.02, y: point.tau + point.sd }
					]
				}
			])
	);
</script>

<div class="w-full">
	<div class="text-muted-foreground mb-2 text-sm">
		{#if data.metadata.uniqueIPCount && data.metadata.uniqueIPCount > 0}
			<p class="text-primary text-xs font-medium">
				✓ Real NetFlow Data Analysis - {data.metadata.uniqueIPCount.toLocaleString()} unique IP addresses
				analyzed
			</p>
		{:else if data.metadata.uniqueIPCount !== -1}
			<p class="text-muted-foreground text-xs">⚠ Using test data from MAAD sample set</p>
		{/if}
	</div>
	<div class="relative h-72 w-full min-w-0 sm:h-96">
		<ChartPlot
			name="Structure function chart"
			{series}
			options={{
				xTitle: 'q',
				yTitle: 'tau(q)',
				xDomain: [-2.1, 4.1],
				xTicks: [-2.1, -1, 0, 1, 2, 3, 4.1],
				zero: false,
				annotations
			}}
			formatTooltip={(points) =>
				`q = ${points[0]?.datum.x.toFixed(3)}\ntau(q): ${points[0]?.datum.y?.toFixed(6)}\nStandard Deviation: ±${points[0]?.datum.sd?.toFixed(6)}`}
		/>
	</div>
</div>
