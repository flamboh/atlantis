<script lang="ts">
	import { onMount } from 'svelte';
	import { Chart } from './annotation-chart-registry';
	import type { StructureFunctionData } from '$lib/types/types';
	import { theme } from '$lib/stores/theme.svelte';

	let { data }: { data: StructureFunctionData } = $props();
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

		if (data?.structureFunction?.length > 0) {
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
		const points = data.structureFunction;
		const { textColor, gridColor, tooltipBackgroundColor, tooltipTextColor, tooltipBorderColor } =
			getChartColors();

		const errorBarAnnotations: Record<
			string,
			{
				type: 'line';
				xMin: number;
				xMax: number;
				yMin: number;
				yMax: number;
				borderColor: string;
				borderWidth: number;
			}
		> = {};

		points.forEach((point, index) => {
			const capWidth = 0.02;

			errorBarAnnotations[`errorBar_${index}`] = {
				type: 'line',
				xMin: point.q,
				xMax: point.q,
				yMin: point.tau - point.sd,
				yMax: point.tau + point.sd,
				borderColor: 'rgba(128, 128, 128, 0.7)',
				borderWidth: 1.5
			};

			errorBarAnnotations[`errorBarTop_${index}`] = {
				type: 'line',
				xMin: point.q - capWidth,
				xMax: point.q + capWidth,
				yMin: point.tau + point.sd,
				yMax: point.tau + point.sd,
				borderColor: 'rgba(128, 128, 128, 0.7)',
				borderWidth: 1.5
			};

			errorBarAnnotations[`errorBarBottom_${index}`] = {
				type: 'line',
				xMin: point.q - capWidth,
				xMax: point.q + capWidth,
				yMin: point.tau - point.sd,
				yMax: point.tau - point.sd,
				borderColor: 'rgba(128, 128, 128, 0.7)',
				borderWidth: 1.5
			};
		});

		const chartData = {
			datasets: [
				{
					label: 'tau(q)',
					data: points.map((p) => ({ x: p.q, y: p.tau })),
					borderColor: 'rgb(59, 130, 246)',
					backgroundColor: 'rgba(59, 130, 246, 0.1)',
					borderWidth: 2,
					pointRadius: 0,
					pointHoverRadius: 0,
					pointBackgroundColor: 'rgb(59, 130, 246)',
					pointBorderColor: 'white',
					pointBorderWidth: 1,
					fill: false,
					tension: 0.1
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
						min: -2.1,
						max: 4.1,
						title: {
							display: true,
							text: 'q',
							color: textColor
						},
						ticks: { color: textColor },
						grid: { color: gridColor }
					},
					y: {
						type: 'linear' as const,
						title: {
							display: true,
							text: 'tau(q)',
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
								`q = ${items[0]?.parsed?.x?.toFixed(3)}`,
							label: (item: {
								dataset: { label: string };
								parsed: { y: number };
								dataIndex: number;
							}) => {
								const value = item.parsed.y.toFixed(6);
								const pointIndex = item.dataIndex;
								const point = points[pointIndex];
								return [
									`${item.dataset.label}: ${value}`,
									`Standard Deviation: ±${point.sd.toFixed(6)}`
								];
							}
						}
					},
					annotation: {
						annotations: errorBarAnnotations
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
