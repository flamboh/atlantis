<script lang="ts">
	import ChartPlot from './ChartPlot.svelte';
	import { plotBounds, type PlotSeries } from './chart-registry';
	import { openTemporalPoint, openTemporalRange } from './temporal-navigation';
	import type { ChartCoverage } from './chart-utils';
	import { groupByBucketDurationMs } from './chart-utils';
	import { formatIpGranularityTick, formatTemporalBucketLabel } from './ip-time-axis';
	import type { GroupByOption } from '#lib/components/netflow/types.ts';
	import type { IpGranularity } from '#lib/types/types.ts';

	export type MetricLineSeries = {
		label: string;
		values: Array<number | null>;
		color: string;
		dash?: number[];
		coverage: ChartCoverage[];
	};
	const props: {
		chartId: string;
		title: string;
		hideTitle?: boolean;
		yAxisTitle: string;
		bucketStarts: number[];
		granularity: IpGranularity;
		groupBy: GroupByOption;
		series: MetricLineSeries[];
		valueFormat?: 'duration' | 'decimal' | 'integer';
		onDrillDown?: (groupBy: GroupByOption, startDate: string, endDate: string) => void;
		onNavigateToFile?: (slug: string) => void;
	} = $props();
	const series = $derived<PlotSeries[]>(
		props.series.map((item) => ({
			label: item.label,
			color: item.color,
			dash: item.dash?.join(' '),
			data: props.bucketStarts.map((start, index) => ({
				x: start,
				y: item.values[index] ?? null,
				coverage: item.coverage[index],
				label: formatTemporalBucketLabel(start, props.granularity)
			}))
		}))
	);
	const bounds = $derived(plotBounds(series, groupByBucketDurationMs(props.groupBy) / 1000));
	function formatValue(value: number) {
		if (props.valueFormat === 'duration')
			return value >= 1000 ? `${(value / 1000).toFixed(2)} s` : `${value.toFixed(1)} ms`;
		if (props.valueFormat === 'integer') return Math.round(value).toLocaleString();
		return value.toFixed(2);
	}
</script>

<section class="flex h-full min-h-0 min-w-0 flex-col" aria-label={props.title}>
	{#if !props.hideTitle}<h3 class="text-foreground px-3 pt-3 text-sm font-semibold">
			{props.title}
		</h3>{/if}
	<div class="relative min-h-0 flex-1">
		<ChartPlot
			name={`${props.title} time series`}
			chartId={props.chartId}
			{series}
			emptyCopy="No data for this metric"
			options={{
				xTitle: `Time (${props.granularity})`,
				yTitle: props.yAxisTitle,
				xDomain: bounds,
				xTicks: props.bucketStarts.filter((start) => start >= bounds[0] && start <= bounds[1]),
				xFormat: (start) => formatIpGranularityTick(start, props.granularity, 0),
				yFormat: formatValue
			}}
			formatTooltip={(points) =>
				[
					points[0]?.datum.label ?? '',
					...points.map((point) => `${point.groupLabel}: ${formatValue(point.datum.y ?? 0)}`)
				].join('\n')}
			onSelect={(point) =>
				openTemporalPoint(point, props.groupBy, props.onDrillDown, props.onNavigateToFile)}
			onRange={(start, end) => openTemporalRange(start, end, props.groupBy, props.onDrillDown)}
		/>
	</div>
</section>
