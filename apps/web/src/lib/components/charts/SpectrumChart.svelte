<script lang="ts">
	import ChartPlot from './ChartPlot.svelte';
	import { finiteSpectrumPoints, paddedSpectrumBounds } from './spectrum-points';
	import type { SpectrumData } from '#lib/types/types.ts';
	let { data }: { data: SpectrumData } = $props();
	const points = $derived(finiteSpectrumPoints(data.spectrum));
	const bounds = $derived.by(() => {
		let min = Infinity;
		let max = -Infinity;
		for (const point of points) {
			min = Math.min(min, point.alpha);
			max = Math.max(max, point.alpha);
		}
		return paddedSpectrumBounds(min, max);
	});
	const series = $derived([
		{
			label: 'f(alpha)',
			color: 'var(--chart-series-7)',
			strokeWidth: 2,
			pointFill: 'var(--chart-series-7)',
			pointStroke: 'var(--card)',
			pointStrokeWidth: 1,
			radius: 3,
			data: points.map((point) => ({ x: point.alpha, y: point.f }))
		}
	]);
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
			name="Multifractal spectrum chart"
			{series}
			emptyCopy="No finite spectrum data for this selection."
			options={{
				xTitle: 'alpha',
				yTitle: 'f(alpha)',
				xDomain: [bounds.min, bounds.max],
				zero: false,
				annotations: [
					{
						color: 'var(--chart-text-color)',
						opacity: 0.5,
						strokeWidth: 1,
						dash: '5 5',
						data: [
							{ x: bounds.min, y: bounds.min },
							{ x: bounds.max, y: bounds.max }
						]
					}
				]
			}}
			formatTooltip={(points) =>
				`alpha = ${points[0]?.datum.x.toFixed(6)}\nf(alpha): ${points[0]?.datum.y?.toFixed(6)}`}
		/>
	</div>
	{#if points.length > 0}<p class="text-muted-foreground mt-1 text-xs">
			Dashed line: f(alpha) = alpha.
		</p>{/if}
</div>
