<script lang="ts">
	import { onMount } from 'svelte';
	import { Chart } from './annotation-chart-registry';
	import type { SpectrumData } from '$lib/types/types';
	import { theme } from '$lib/stores/theme.svelte';

	let { data }: { data: SpectrumData } = $props();
	let chartCanvas: HTMLCanvasElement;
	let chart: Chart | null = null;

	onMount(() => {
		return () => {
			destroyChart();
		};
	});

	$effect(() => {
		if (!chartCanvas?.parentElement) {
			return;
		}

		const observer = new ResizeObserver(() => {
			chart?.resize();
		});
		observer.observe(chartCanvas.parentElement);

		return () => {
			observer.disconnect();
		};
	});

	$effect(() => {
		void theme.dark;
		if (!chartCanvas) {
			return;
		}

		if (data?.spectrum?.length > 0) {
			updateChart();
			return;
		}

		destroyChart();
	});

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

	function destroyChart() {
		if (chart) {
			chart.destroy();
			chart = null;
		}
	}

	function updateChart() {
		const points = data.spectrum;
		const { textColor, gridColor, tooltipBackgroundColor, tooltipTextColor, tooltipBorderColor } =
			getChartColors();

		const alphaValues = points.map((p) => p.alpha);
		const minAlpha = Math.min(...alphaValues);
		const maxAlpha = Math.max(...alphaValues);

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
				},
				{
					label: 'y = x (reference)',
					data: [
						{
							x: minAlpha - 0.02 * (maxAlpha - minAlpha),
							y: minAlpha - 0.02 * (maxAlpha - minAlpha)
						},
						{
							x: maxAlpha + 0.02 * (maxAlpha - minAlpha),
							y: maxAlpha + 0.02 * (maxAlpha - minAlpha)
						}
					],
					borderColor: 'rgba(128, 128, 128, 0.5)',
					borderWidth: 1,
					borderDash: [5, 5],
					pointRadius: 0,
					pointHoverRadius: 0,
					fill: false,
					tension: 0
				}
			]
		};

		const config = {
			type: 'line' as const,
			data: chartData,
			options: {
				responsive: true,
				maintainAspectRatio: false,
				animation: false as const,
				scales: {
					x: {
						type: 'linear' as const,
						min: minAlpha - 0.02 * (maxAlpha - minAlpha),
						max: maxAlpha + 0.02 * (maxAlpha - minAlpha),
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
						mode: 'index' as const,
						intersect: false,
						backgroundColor: tooltipBackgroundColor,
						titleColor: tooltipTextColor,
						bodyColor: tooltipTextColor,
						borderColor: tooltipBorderColor,
						borderWidth: 1,
						callbacks: {
							title: (items: { parsed: { x: number } }[]) =>
								`alpha = ${items[0]?.parsed?.x?.toFixed(6)}`,
							label: (item: {
								dataset: { label: string };
								parsed: { y: number };
								dataIndex: number;
							}) => {
								const value = item.parsed.y.toFixed(6);
								return `${item.dataset.label}: ${value}`;
							}
						}
					},
					verticalCrosshair: {
						enabled: true,
						tooltip: {
							enabled: false
						}
					}
				},
				interaction: {
					mode: 'index' as const,
					intersect: false
				}
			}
		};

		if (!chart) {
			// eslint-disable-next-line @typescript-eslint/no-explicit-any
			chart = new Chart(chartCanvas, config as any);
			return;
		}

		chart.data = chartData;
		// eslint-disable-next-line @typescript-eslint/no-explicit-any
		chart.options = config.options as any;
		chart.update('none');
	}
</script>

<div class="w-full">
	<div class="mb-2 text-sm text-gray-600 dark:text-gray-400">
		{#if data.metadata.uniqueIPCount && data.metadata.uniqueIPCount > 0}
			<p class="text-xs font-medium text-green-600 dark:text-green-400">
				✓ Real NetFlow Data Analysis - {data.metadata.uniqueIPCount.toLocaleString()} unique IP addresses
				analyzed
			</p>
		{:else if data.metadata.uniqueIPCount !== -1}
			<p class="text-xs text-amber-600 dark:text-amber-400">
				⚠ Using test data from MAAD sample set
			</p>
		{/if}
	</div>
	<div class="relative h-72 w-full min-w-0 sm:h-96">
		<canvas bind:this={chartCanvas}></canvas>
	</div>
</div>
