<script lang="ts">
	import type { ChartConfiguration } from 'chart.js';
	import { finiteSpectrumPoints, paddedSpectrumBounds } from './spectrum-points';
	import { Chart } from './annotation-chart-registry';
	import type { SpectrumData } from '#lib/types/types.ts';
	import { theme } from '#lib/stores/theme.svelte.ts';

	let { data }: { data: SpectrumData } = $props();
	const points = $derived(finiteSpectrumPoints(data.spectrum));

	function attachChart(canvas: HTMLCanvasElement) {
		void theme.dark;
		const chart = new Chart(canvas, buildConfig());
		return () => chart.destroy();
	}

	function getChartColors() {
		const style = getComputedStyle(document.documentElement);
		return {
			textColor: style.getPropertyValue('--chart-text-color').trim(),
			gridColor: style.getPropertyValue('--chart-grid-color').trim(),
			tooltipBackgroundColor: style.getPropertyValue('--chart-tooltip-bg').trim(),
			tooltipTextColor: style.getPropertyValue('--chart-tooltip-text-color').trim(),
			tooltipBorderColor: style.getPropertyValue('--chart-tooltip-border-color').trim()
		};
	}

	function buildConfig(): ChartConfiguration<'line'> {
		const { textColor, gridColor, tooltipBackgroundColor, tooltipTextColor, tooltipBorderColor } =
			getChartColors();

		const alphaValues = points.map((p) => p.alpha);
		const minAlpha = Math.min(...alphaValues);
		const maxAlpha = Math.max(...alphaValues);
		const bounds = paddedSpectrumBounds(minAlpha, maxAlpha);

		const chartData = {
			datasets: [
				{
					label: 'f(alpha)',
					data: points.map((p) => ({ x: p.alpha, y: p.f })),
					borderColor: 'rgb(147, 51, 234)',
					backgroundColor: 'rgba(147, 51, 234, 0.1)',
					borderWidth: 2,
					pointRadius: 3,
					pointHoverRadius: 5,
					pointBackgroundColor: 'rgb(147, 51, 234)',
					pointBorderColor: 'white',
					pointBorderWidth: 1,
					fill: false,
					tension: 0.3
				}
			]
		};

		return {
			type: 'line' as const,
			data: chartData,
			options: {
				responsive: true,
				maintainAspectRatio: false,
				animation: false as const,
				scales: {
					x: {
						type: 'linear' as const,
						min: bounds.min,
						max: bounds.max,
						title: {
							display: true,
							text: 'alpha',
							color: textColor
						},
						ticks: { color: textColor },
						grid: { color: gridColor }
					},
					y: {
						type: 'linear' as const,
						title: {
							display: true,
							text: 'f(alpha)',
							color: textColor
						},
						position: 'left' as const,
						ticks: { color: textColor },
						grid: { color: gridColor }
					}
				},
				plugins: {
					legend: {
						display: true,
						position: 'top' as const,
						labels: { color: textColor }
					},
					tooltip: {
						mode: 'nearest' as const,
						intersect: false,
						backgroundColor: tooltipBackgroundColor,
						titleColor: tooltipTextColor,
						bodyColor: tooltipTextColor,
						borderColor: tooltipBorderColor,
						borderWidth: 1,
						callbacks: {
							title: (items) => `alpha = ${items[0]?.parsed?.x?.toFixed(6)}`,
							label: (item) => {
								const value = item.parsed.y?.toFixed(6);
								return `${item.dataset.label}: ${value}`;
							}
						}
					},
					annotation: {
						annotations: {
							reference: {
								type: 'line',
								xMin: bounds.min,
								xMax: bounds.max,
								yMin: bounds.min,
								yMax: bounds.max,
								borderColor: 'rgba(128, 128, 128, 0.5)',
								borderWidth: 1,
								borderDash: [5, 5]
							}
						}
					},
					verticalCrosshair: false
				},
				interaction: {
					mode: 'nearest' as const,
					intersect: false
				}
			}
		};
	}
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
		{#if points.length > 0}
			<canvas {@attach attachChart} aria-label="Multifractal spectrum chart"></canvas>
		{:else}
			<p class="text-muted-foreground flex h-full items-center justify-center">
				No finite spectrum data for this selection.
			</p>
		{/if}
	</div>
	{#if points.length > 0}
		<p class="text-muted-foreground mt-1 text-xs">Dashed line: f(alpha) = alpha.</p>
	{/if}
</div>
