<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import type { ChartPoint } from '@tanstack/charts';
	import ChartPlot from './ChartPlot.svelte';
	import { plotBounds, type PlotPoint, type PlotSeries } from './chart-registry';
	import {
		formatLabels,
		formatNetflowTick,
		formatNumber,
		getXAxisTitle,
		groupByBucketDurationMs
	} from './chart-utils';
	import { openTemporalPoint, openTemporalRange } from './temporal-navigation';
	import { NETFLOW_DATA_OPTION_FIELDS } from '#lib/components/netflow/constants.ts';
	import type {
		NetflowDataPoint,
		GroupByOption,
		ChartTypeOption,
		DataOption
	} from '#lib/components/netflow/types.ts';

	let {
		results,
		groupBy,
		chartType,
		dataOptions,
		onDrillDown,
		onNavigateToFile
	}: {
		results: NetflowDataPoint[];
		groupBy: GroupByOption;
		chartType: ChartTypeOption;
		dataOptions: DataOption[];
		onDrillDown?: (groupBy: GroupByOption, startDate: string, endDate: string) => void;
		onNavigateToFile?: (slug: string) => void;
	} = $props();
	const colors = [
		'rgb(75, 192, 192)',
		'rgb(255, 99, 132)',
		'rgb(54, 162, 235)',
		'rgb(255, 206, 86)',
		'rgb(153, 102, 255)',
		'rgb(255, 159, 64)',
		'rgb(255, 99, 71)',
		'rgb(0, 206, 209)',
		'rgb(60, 179, 113)',
		'rgb(218, 112, 214)',
		'rgb(255, 215, 0)',
		'rgb(128, 0, 128)'
	];
	const labels = $derived(formatLabels(results, groupBy));
	const series = $derived<PlotSeries[]>(
		dataOptions
			.filter((option) => option.checked)
			.map((option, index) => ({
				label: option.label,
				color: colors[index % colors.length],
				data: results.map((result, index) => ({
					x: result.bucketStart,
					y: result.data?.[NETFLOW_DATA_OPTION_FIELDS[option.index]] ?? null,
					coverage: result.coverage,
					label: labels[index]
				}))
			}))
	);
	const bounds = $derived(plotBounds(series, groupByBucketDurationMs(groupBy) / 1000));
	const visibleStarts = $derived(
		results
			.map((result) => result.bucketStart)
			.filter((start) => start >= bounds[0] && start <= bounds[1])
	);
	const allBytes = $derived(
		dataOptions.filter((option) => option.checked).every((option) => option.label.includes('Bytes'))
	);

	function formatMetric(value: number, label: string) {
		if (label.includes('Bytes')) {
			for (const [power, unit] of [
				[5, 'PB'],
				[4, 'TB'],
				[3, 'GB'],
				[2, 'MB'],
				[1, 'KB']
			] as const) {
				if (value >= 1024 ** power) return `${(value / 1024 ** power).toFixed(1)} ${unit}`;
			}
			return `${value.toLocaleString()} bytes`;
		}
		return value.toLocaleString();
	}
	function tooltip(points: readonly ChartPoint<PlotPoint, number, number>[]) {
		const bucket = results.find((result) => result.bucketStart === points[0]?.datum.x);
		const lines = [
			points[0]?.datum.label ?? '',
			...points.map(
				(point) => `${point.groupLabel}: ${formatMetric(point.datum.y ?? 0, point.groupLabel)}`
			)
		];
		for (const [family, label] of [
			['flows', 'Flows'],
			['packets', 'Packets'],
			['bytes', 'Bytes']
		] as const) {
			if (bucket?.data && points.some((point) => point.groupLabel.includes(label)))
				lines.push(`Total ${label}: ${formatMetric(bucket.data[family], label)}`);
		}
		return lines.join('\n');
	}
	function navigate(slug: string) {
		if (onNavigateToFile) onNavigateToFile(slug);
		else void goto(resolve('/netflow/files/[slug]', { slug }));
	}
</script>

<ChartPlot
	retainEmptySurface
	name="Traffic overview chart"
	chartId="netflow"
	{series}
	options={{
		kind: chartType,
		xTitle: getXAxisTitle(groupBy),
		yTitle: 'Value',
		xDomain: bounds,
		xTicks: visibleStarts,
		xFormat: (start) =>
			formatNetflowTick(
				groupBy,
				labels[results.findIndex((result) => result.bucketStart === start)],
				0
			),
		yFormat: (value) => (allBytes ? formatMetric(value, 'Bytes') : formatNumber(value)),
		compact: true
	}}
	formatTooltip={tooltip}
	emptyCopy={series.length
		? 'No traffic observations for the selected window.'
		: 'Select at least one metric to display.'}
	onSelect={(point) => openTemporalPoint(point, groupBy, onDrillDown, navigate)}
	onRange={(start, end) => openTemporalRange(start, end, groupBy, onDrillDown)}
/>
