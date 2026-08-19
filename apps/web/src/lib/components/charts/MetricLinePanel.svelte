<script lang="ts">
	import { onDestroy } from 'svelte';
	import { Chart } from './chart-registry';
	import type { ChartConfiguration } from 'chart.js';
	import type { IpGranularity } from '$lib/types/types';
	import { formatIpGranularityTick, formatTemporalBucketLabel } from './ip-time-axis';
	import { theme } from '$lib/stores/theme.svelte';
	import {
		getCoveragePointStyle,
		getCoverageTooltipLines,
		isCoverageSegmentDashed,
		type ChartCoverage
	} from './chart-utils';

	export type MetricLineSeries = {
		label: string;
		values: Array<number | null>;
		color: string;
		dash?: number[];
		coverage: ChartCoverage[];
	};

	type MetricLinePoint = {
		x: number;
		y: number | null;
		bucketStart: number;
		coverage: ChartCoverage;
	};

	const props = $props<{
		title: string;
		yAxisTitle: string;
		bucketStarts: number[];
		granularity: IpGranularity;
		series: MetricLineSeries[];
		valueFormat?: 'duration' | 'decimal' | 'integer';
	}>();

	let canvas = $state<HTMLCanvasElement | null>(null);
	let chart: Chart<'line', MetricLinePoint[], string> | null = null;

	function formatValue(value: number): string {
		if (props.valueFormat === 'duration') {
			return value >= 1000 ? `${(value / 1000).toFixed(2)} s` : `${value.toFixed(1)} ms`;
		}
		if (props.valueFormat === 'integer') {
			return Math.round(value).toLocaleString();
		}
		return value.toFixed(2);
	}

	function colors() {
		const style = getComputedStyle(document.documentElement);
		return {
			text: style.getPropertyValue('--chart-text-color').trim(),
			grid: style.getPropertyValue('--chart-grid-color').trim(),
			tooltip: style.getPropertyValue('--chart-tooltip-bg').trim(),
			tooltipText: style.getPropertyValue('--chart-tooltip-text-color').trim(),
			tooltipBorder: style.getPropertyValue('--chart-tooltip-border-color').trim()
		};
	}

	function renderChart() {
		if (!canvas || props.bucketStarts.length === 0 || props.series.length === 0) {
			chart?.destroy();
			chart = null;
			return;
		}
		const palette = colors();
		const datasets = props.series.map((series: MetricLineSeries) => {
			const data: MetricLinePoint[] = props.bucketStarts.map(
				(bucketStart: number, index: number) => {
					const coverage = series.coverage[index] ?? {
						state: 'unknown' as const,
						observedUnits: 0,
						expectedUnits: 0
					};
					return {
						x: bucketStart,
						y: series.values[index] ?? null,
						bucketStart,
						coverage
					};
				}
			);
			const pointStyles = data.map((point: MetricLinePoint) =>
				getCoveragePointStyle(point.coverage, series.color)
			);

			return {
				label: series.label,
				data,
				borderColor: series.color,
				backgroundColor: series.color,
				borderDash: series.dash ?? [],
				pointRadius: data.map((point: MetricLinePoint, index: number) =>
					point.y === null ? 0 : (pointStyles[index]?.radius ?? 0)
				),
				pointBackgroundColor: pointStyles.map(
					(style: (typeof pointStyles)[number]) => style.backgroundColor
				),
				pointBorderColor: pointStyles.map(
					(style: (typeof pointStyles)[number]) => style.borderColor
				),
				pointBorderWidth: pointStyles.map(
					(style: (typeof pointStyles)[number]) => style.borderWidth
				),
				pointHoverRadius: 4,
				spanGaps: false,
				segment: {
					borderDash: (context: { p0: { raw: unknown }; p1: { raw: unknown } }) =>
						isCoverageSegmentDashed(
							context.p0.raw as { coverage?: ChartCoverage },
							context.p1.raw as { coverage?: ChartCoverage }
						)
							? [6, 4]
							: (series.dash ?? [])
				},
				parsing: false as const,
				tension: 0.25
			};
		});
		const labels = props.bucketStarts.map((bucketStart: number) =>
			formatTemporalBucketLabel(bucketStart, props.granularity)
		);
		chart?.destroy();
		const config: ChartConfiguration<'line', MetricLinePoint[], string> = {
			type: 'line',
			data: { labels, datasets },
			options: {
				responsive: true,
				maintainAspectRatio: false,
				animation: false,
				interaction: { mode: 'index', intersect: false },
				plugins: {
					legend: { position: 'top', labels: { color: palette.text } },
					tooltip: {
						backgroundColor: palette.tooltip,
						titleColor: palette.tooltipText,
						bodyColor: palette.tooltipText,
						borderColor: palette.tooltipBorder,
						borderWidth: 1,
						callbacks: {
							afterLabel: (context: { raw: unknown }) => {
								const raw = context.raw;
								if (typeof raw !== 'object' || raw === null || !('coverage' in raw)) return [];
								return getCoverageTooltipLines((raw as { coverage: ChartCoverage }).coverage);
							},
							label: (context: { dataset: { label?: string }; parsed: { y: number | null } }) =>
								`${context.dataset.label ?? ''}: ${context.parsed.y === null ? 'No data' : formatValue(context.parsed.y)}`
						}
					}
				},
				scales: {
					x: {
						type: 'linear',
						min: props.bucketStarts[0],
						max: props.bucketStarts[props.bucketStarts.length - 1],
						ticks: {
							color: palette.text,
							autoSkip: false,
							maxRotation: 0,
							callback: (value: string | number) =>
								formatIpGranularityTick(Number(value), props.granularity, 0)
						},
						grid: { color: palette.grid }
					},
					y: {
						beginAtZero: true,
						title: { display: true, text: props.yAxisTitle, color: palette.text },
						ticks: {
							color: palette.text,
							callback: (value: string | number) => formatValue(Number(value))
						},
						grid: { color: palette.grid }
					}
				}
			}
		};
		chart = new Chart(canvas, config);
	}

	$effect(() => {
		void theme.dark;
		renderChart();
	});

	onDestroy(() => chart?.destroy());
</script>

<section class="flex min-h-72 flex-col" aria-label={props.title}>
	<h4 class="mb-2 text-sm font-semibold text-gray-800 dark:text-gray-200">{props.title}</h4>
	{#if props.bucketStarts.length === 0 || props.series.length === 0}
		<div class="flex flex-1 items-center justify-center text-sm text-gray-500 dark:text-gray-400">
			No data for this metric
		</div>
	{:else}
		<div class="relative min-h-64 flex-1">
			<canvas bind:this={canvas} aria-label={`${props.title} time series`}></canvas>
		</div>
		<p class="sr-only">
			{props.series.length} series across {props.bucketStarts.length} time buckets.
		</p>
	{/if}
</section>
