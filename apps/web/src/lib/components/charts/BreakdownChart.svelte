<script lang="ts" generics="Kind extends BreakdownChartKind">
	import { goto } from '$app/navigation';
	import type { GroupByOption, RouterConfig } from '#lib/components/netflow/types.ts';
	import {
		DEFAULT_MAAD_IP_VERSION,
		MAAD_IP_VERSION_OPTIONS,
		MAAD_MEASURE_OPTIONS,
		type FlowDirection,
		type IpGranularity,
		type IpMetricKey,
		type MaadIpVersion,
		type MaadMeasure,
		type ProtocolMetricKey,
		type SpectrumPoint,
		type TimeBucket
	} from '#lib/types/types.ts';
	import type { SpectrumStatsPayload } from '#lib/types/spectrum-stats.ts';
	import type { DimensionMetricKey } from '#lib/types/dimension-stats.ts';
	import { navigateToNetflowFile } from '#lib/utils/netflow-file-navigation.ts';
	import { dateStringToEpochPST, formatDateAsPSTDateString } from '#lib/utils/timezone.ts';
	import { ensureCachedWindow, readCachedWindow, type TimeRange } from '#lib/utils/window-cache.ts';
	import { Checkbox } from '#lib/components/ui/checkbox/index.ts';
	import { Skeleton } from '#lib/components/ui/skeleton/index.ts';
	import SegmentedControl from '#lib/components/common/SegmentedControl.svelte';
	import ChartCard from './ChartCard.svelte';
	import { getSourceLineDash } from './flow-characteristics';
	import ChartPlot from './ChartPlot.svelte';
	import { plotBounds, finitePoint, type PlotSeries, type PlotOptions } from './chart-registry';
	import { finiteSpectrumPoints, paddedSpectrumBounds } from './spectrum-points';
	import {
		BREAKDOWN_CHART_CONFIGS,
		DIMENSION_ORDER_OPTIONS,
		DIMENSION_SIDE_OPTIONS,
		dimensionMetricKey,
		readLineMetric,
		splitDimensionMetricKey,
		type BreakdownChartKind,
		type BreakdownChartConfig,
		type BreakdownMetricKey,
		type DimensionOrder,
		type DimensionSide,
		type LineBucketData
	} from './breakdown-chart-config';
	import { formatNumber, groupByBucketDurationMs, getChartBucketCoverage } from './chart-utils';
	import { formatTemporalBucketLabel, formatIpGranularityTick } from './ip-time-axis';
	import { openTemporalPoint, openTemporalRange } from './temporal-navigation';
	const IP_TO_GROUP_BY: Record<IpGranularity, GroupByOption> = {
		'1d': 'date',
		'1h': 'hour',
		'30m': '30min',
		'10m': '10min',
		'5m': '5min'
	};
	type MetricsForKind<ChartKind extends BreakdownChartKind> = ChartKind extends 'ip'
		? IpMetricKey[]
		: ChartKind extends 'protocol'
			? ProtocolMetricKey[]
			: ChartKind extends 'dimensions'
				? DimensionMetricKey[]
				: never[];

	const props = $props<{
		kind: Kind;
		dataset?: string;
		startDate?: string;
		endDate?: string;
		granularity?: IpGranularity;
		router?: string;
		addressType?: 'sa' | 'da';
		ipVersion?: MaadIpVersion;
		measure?: MaadMeasure;
		unavailableCopy?: string | null;
		unavailableSideCopy?: Partial<Record<DimensionSide, string>>;
		availableRouters?: string[];
		routers?: RouterConfig;
		activeMetrics?: MetricsForKind<Kind>;
		direction?: FlowDirection;
		onDrillDown?: (payload: { groupBy: GroupByOption; startDate: string; endDate: string }) => void;
		onRouterChange?: (payload: { router: string }) => void;
		onAddressTypeChange?: (payload: { addressType: 'sa' | 'da' }) => void;
		onMetricsChange?: (payload: { metrics: MetricsForKind<Kind> }) => void;
	}>();
	function getConfig(kind: BreakdownChartKind): BreakdownChartConfig {
		return BREAKDOWN_CHART_CONFIGS[kind];
	}

	const config = $derived(getConfig(props.kind));

	const today = new Date();
	const formatDate = (date: Date): string => formatDateAsPSTDateString(date);

	type BreakdownBucketData = LineBucketData | SpectrumStatsPayload;
	type BreakdownChartBucket = TimeBucket<BreakdownBucketData>;
	type CachedBreakdownBucket = {
		router: string;
		bucket: BreakdownChartBucket;
	};

	const currentRouter = $derived(
		props.availableRouters?.includes(props.router ?? '')
			? (props.router ?? '')
			: (props.availableRouters?.[0] ?? '')
	);
	const currentGranularity = $derived<IpGranularity>(
		props.granularity ?? config.defaultGranularity
	);

	let addressType = $derived(props.addressType ?? 'sa');
	const ipVersion = $derived<MaadIpVersion>(props.ipVersion ?? DEFAULT_MAAD_IP_VERSION);
	type FilterInputs = {
		startDate: string;
		endDate: string;
		granularity: IpGranularity;
		routers: string[];
		direction: FlowDirection;
		ipVersion?: MaadIpVersion;
		measure?: MaadMeasure;
		addressSide?: 'source' | 'destination';
	};

	const filters = $derived<FilterInputs>({
		startDate: props.startDate ?? '2025-01-01',
		endDate: props.endDate ?? formatDate(today),
		granularity: currentGranularity,
		routers:
			props.kind === 'spectrum'
				? currentRouter
					? [currentRouter]
					: []
				: deriveSelectedRouters(props.routers),
		direction: props.direction ?? 'all',
		...(config.usesMaad ? { ipVersion, measure: props.measure } : {}),
		...(props.kind === 'spectrum'
			? { addressSide: addressType === 'sa' ? ('source' as const) : ('destination' as const) }
			: {})
	});
	const requestKey = $derived(
		JSON.stringify({
			dataset: props.dataset ?? '',
			kind: props.kind,
			unavailable: props.unavailableCopy,
			...filters
		})
	);
	let settled = $state.raw<{
		key: string;
		records: CachedBreakdownBucket[];
		error: string | null;
	} | null>(null);

	const cachedBuckets = $derived(settled?.key === requestKey ? settled.records : []);
	const buckets = $derived(
		cachedBuckets
			.filter((record) => props.kind !== 'spectrum' || record.router === currentRouter)
			.map((record) => record.bucket)
	);
	let activeMetrics = $derived<BreakdownMetricKey[]>(props.activeMetrics ?? config.defaultMetrics);
	const loading = $derived(
		!props.unavailableCopy && filters.routers.length > 0 && settled?.key !== requestKey
	);
	const error = $derived(
		filters.routers.length === 0
			? config.noSourceCopy
			: settled?.key === requestKey
				? settled.error
				: null
	);
	const selectedSide = $derived<DimensionSide | null>(
		props.kind === 'spectrum'
			? addressType
			: props.kind === 'dimensions'
				? splitDimensionMetricKey(
						(activeMetrics[0] ?? config.defaultMetrics[0]) as DimensionMetricKey
					).side
				: null
	);
	const sideUnavailableCopy = $derived(
		selectedSide ? (props.unavailableSideCopy?.[selectedSide] ?? null) : null
	);

	const maadSubtitle = $derived(
		config.usesMaad && !props.unavailableCopy
			? [
					MAAD_MEASURE_OPTIONS.find((option) => option.value === props.measure)?.label,
					MAAD_IP_VERSION_OPTIONS.find((option) => option.value === ipVersion)?.label
				]
					.filter(Boolean)
					.join(' · ')
			: null
	);

	function deriveSelectedRouters(routers: RouterConfig | undefined): string[] {
		return Object.entries(routers ?? {})
			.filter(([, enabled]) => enabled)
			.map(([name]) => name.trim())
			.filter(Boolean)
			.sort();
	}
	function pointsForBucket(bucket: BreakdownChartBucket): SpectrumPoint[] {
		if (!bucket.data || !('spectrumSa' in bucket.data)) return [];
		return finiteSpectrumPoints(
			addressType === 'sa' ? bucket.data.spectrumSa : bucket.data.spectrumDa
		);
	}
	function handleMetricToggle(metric: BreakdownMetricKey) {
		const next = activeMetrics.includes(metric)
			? activeMetrics.filter((value) => value !== metric)
			: [...activeMetrics, metric];
		activeMetrics = next;
		props.onMetricsChange?.({ metrics: next as MetricsForKind<Kind> });
	}
	function handleRouterChange(router: string) {
		props.onRouterChange?.({ router });
	}
	function handleAddressTypeChange(side: 'sa' | 'da') {
		addressType = side;
		props.onAddressTypeChange?.({ addressType: side });
	}
	function handleDimensionChange(side: DimensionSide, order: DimensionOrder) {
		activeMetrics = [dimensionMetricKey(side, order)];
		props.onMetricsChange?.({ metrics: activeMetrics as MetricsForKind<Kind> });
	}
	function toEpochSeconds(date: string, end = false) {
		return dateStringToEpochPST(date, end);
	}
	const bucketStarts = $derived(
		[...new Set(buckets.map((bucket) => bucket.bucketStart))].sort((a, b) => a - b)
	);
	const bucketLabels = $derived(
		new Map(
			bucketStarts.map((start) => [start, formatTemporalBucketLabel(start, currentGranularity)])
		)
	);
	const series = $derived.by((): PlotSeries[] => {
		if (props.kind === 'spectrum')
			return [
				{
					label: 'Spectrum',
					color: 'var(--chart-series-1)',
					data: buckets.flatMap((bucket) =>
						pointsForBucket(bucket).map((point) => ({
							x: bucket.bucketStart,
							y: point.alpha,
							f: point.f,
							label: bucketLabels.get(bucket.bucketStart),
							coverage: bucket.coverage
						}))
					)
				}
			];
		const routers = [...new Set(cachedBuckets.map((record) => record.router))].sort();
		const records = new Map(
			cachedBuckets.map((record) => [
				`${record.router}-${record.bucket.bucketStart}`,
				record.bucket
			])
		);
		return routers.flatMap((router, routerIndex) =>
			config.metrics
				.filter((metric) => activeMetrics.includes(metric.key))
				.map((metric) => {
					const color = metric.color;
					const slot =
						Object.keys(props.routers ?? {})
							.sort()
							.indexOf(router) + 1;
					return {
						label: config.seriesByRouter ? router : `${router} · ${metric.seriesLabel}`,
						color: config.seriesByRouter
							? `var(--chart-series-${slot > 0 && slot <= 8 ? slot : 'other'})`
							: (color ?? 'var(--chart-series-1)'),
						dash: config.seriesByRouter
							? undefined
							: getSourceLineDash(routerIndex, routers.length > 1).join(' '),
						data: bucketStarts.map((start) => {
							const bucket = records.get(`${router}-${start}`);
							return {
								x: start,
								y: readLineMetric((bucket?.data as LineBucketData | null) ?? null, metric.key),
								label: bucketLabels.get(start),
								coverage: getChartBucketCoverage(bucket) ?? {
									state: 'unknown',
									observedUnits: 0,
									expectedUnits: 0
								}
							};
						})
					};
				})
		);
	});
	const hasData = $derived(series.some((item) => item.data.some(finitePoint)));
	const bounds = $derived(
		plotBounds(series, groupByBucketDurationMs(IP_TO_GROUP_BY[currentGranularity]) / 1000)
	);
	const chartOptions = $derived.by((): PlotOptions => {
		const points =
			props.kind === 'spectrum' ? series.flatMap((item) => item.data).filter(finitePoint) : [];
		let minAlpha = Infinity;
		let maxAlpha = -Infinity;
		let minF = Infinity;
		let maxF = -Infinity;
		for (const point of points) {
			minAlpha = Math.min(minAlpha, point.y ?? Infinity);
			maxAlpha = Math.max(maxAlpha, point.y ?? -Infinity);
			minF = Math.min(minF, point.f ?? Infinity);
			maxF = Math.max(maxF, point.f ?? -Infinity);
		}
		const alpha = paddedSpectrumBounds(minAlpha, maxAlpha);
		return {
			kind: props.kind === 'spectrum' ? 'scatter' : 'line',
			xTitle: `Time (${currentGranularity})`,
			yTitle: config.seriesByRouter
				? (config.metrics.find((metric) => activeMetrics.includes(metric.key))?.label ??
					config.yAxisTitle)
				: config.yAxisTitle,
			xDomain: bounds,
			xTicks: bucketStarts.filter((start) => start >= bounds[0] && start <= bounds[1]),
			xFormat: (start) => formatIpGranularityTick(start, currentGranularity, 0),
			yFormat: config.formatYAxisTicks
				? formatNumber
				: config.fitYAxisToData
					? (value) => value.toFixed(2)
					: undefined,
			zero: !config.fitYAxisToData,
			legend: props.kind !== 'spectrum',
			...(props.kind === 'spectrum' && points.length
				? {
						yDomain: [alpha.min, alpha.max],
						colorDomain: minF === maxF ? [minF - 0.5, maxF + 0.5] : [minF, maxF]
					}
				: {})
		};
	});
	function drilldown(groupBy: GroupByOption, startDate: string, endDate: string) {
		props.onDrillDown?.({ groupBy, startDate, endDate });
	}
	function file(slug: string) {
		void navigateToNetflowFile(
			goto,
			slug,
			props.dataset,
			props.direction ?? 'all',
			props.ipVersion,
			props.measure
		);
	}
	async function loadData(filters: FilterInputs, dataset: string, signal: AbortSignal) {
		const requestedRange: TimeRange = {
			start: toEpochSeconds(filters.startDate),
			end: toEpochSeconds(filters.endDate, true)
		};
		const params = new URLSearchParams({
			dataset,
			granularity: filters.granularity,
			routers: filters.routers.join(','),
			direction: filters.direction,
			...(filters.ipVersion !== undefined ? { ipVersion: String(filters.ipVersion) } : {}),
			...(filters.measure !== undefined ? { measure: filters.measure } : {}),
			...(filters.addressSide ? { addressSide: filters.addressSide } : {})
		});
		const cacheKey = `${config.endpoint}?${params}`;
		await ensureCachedWindow<CachedBreakdownBucket>({
			key: cacheKey,
			requestedRange,
			signal,
			fetchRange: async (range, signal) => {
				const response = await fetch(
					`${config.endpoint}?${new URLSearchParams({
						...Object.fromEntries(params.entries()),
						startDate: range.start.toString(),
						endDate: range.end.toString()
					})}`,
					{ signal }
				);
				if (!response.ok) throw new Error((await response.text()) || config.fetchErrorCopy);
				const data = (await response.json()) as {
					timelines: Array<{ router: string; buckets: BreakdownChartBucket[] }>;
				};
				return data.timelines.flatMap((timeline) =>
					timeline.buckets.map((bucket) => ({ router: timeline.router, bucket }))
				);
			},
			getRecordKey: (record) => `${record.router}-${record.bucket.bucketStart}`,
			compareRecords: (left, right) =>
				left.bucket.bucketStart - right.bucket.bucketStart ||
				left.router.localeCompare(right.router)
		});
		return readCachedWindow<CachedBreakdownBucket>(
			cacheKey,
			requestedRange,
			(record, range) =>
				record.bucket.bucketStart >= range.start && record.bucket.bucketStart < range.end
		);
	}

	$effect(() => {
		const key = requestKey;
		if (props.unavailableCopy || filters.routers.length === 0) return;
		const controller = new AbortController();
		loadData(filters, props.dataset ?? '', controller.signal).then(
			(records) => {
				if (!controller.signal.aborted) settled = { key, records, error: null };
			},
			(reason: unknown) => {
				if (!controller.signal.aborted)
					settled = {
						key,
						records: [],
						error: reason instanceof Error ? reason.message : config.unexpectedErrorCopy
					};
			}
		);
		return () => controller.abort();
	});
</script>

<ChartCard
	title={config.title}
	subtitle={maadSubtitle}
	size={props.kind === 'spectrum' ? 'spectrum' : 'default'}
	unavailableCopy={props.unavailableCopy ?? null}
	selectionUnavailableCopy={sideUnavailableCopy}
	{loading}
	{error}
	noMetrics={props.kind === 'spectrum'
		? buckets.length > 0 && !hasData
		: activeMetrics.length === 0}
	empty={buckets.length === 0 || !hasData}
	loadingCopy={config.loadingCopy}
	noMetricsCopy={props.kind === 'spectrum'
		? `No ${addressType === 'sa' ? 'source' : 'destination'} spectrum data for the selected source.`
		: config.noMetricsCopy}
	emptyCopy={config.emptyCopy}
>
	{#snippet controls()}
		{#if props.kind === 'spectrum'}
			<div class="flex flex-wrap items-center gap-3">
				{#if (props.availableRouters ?? []).length === 0}
					<Skeleton class="h-9 w-48" aria-hidden="true" />
				{:else}
					<SegmentedControl
						options={(props.availableRouters ?? []).map((routerName: string) => ({
							value: routerName,
							label: routerName
						}))}
						value={props.router ?? null}
						onValueChange={handleRouterChange}
						ariaLabel="Spectrum source"
						class="flex flex-wrap"
						buttonClass="w-auto sm:min-w-20"
					/>
				{/if}
				<SegmentedControl
					options={DIMENSION_SIDE_OPTIONS}
					value={addressType}
					onValueChange={handleAddressTypeChange}
					ariaLabel="Spectrum address side"
					class="grid-cols-2"
					buttonClass="sm:min-w-20"
				/>
			</div>
		{:else if config.seriesByRouter}
			{@const selected = splitDimensionMetricKey(
				(activeMetrics[0] ?? config.defaultMetrics[0]) as DimensionMetricKey
			)}
			<div class="flex flex-wrap items-center gap-3">
				<SegmentedControl
					options={DIMENSION_SIDE_OPTIONS}
					value={selected.side}
					onValueChange={(side) => handleDimensionChange(side, selected.order)}
					ariaLabel="MAAD address side"
					class="grid-cols-2"
					buttonClass="sm:min-w-20"
				/>
				<SegmentedControl
					options={DIMENSION_ORDER_OPTIONS}
					value={selected.order}
					onValueChange={(order) => handleDimensionChange(selected.side, order)}
					ariaLabel="MAAD dimension"
					class="grid-cols-3"
					buttonClass="sm:min-w-12"
				/>
			</div>
		{:else}
			<div class="flex flex-wrap items-center gap-4">
				{#each config.metrics as metric (metric.key)}
					<label class="text-foreground flex cursor-pointer items-center gap-2 text-sm">
						<Checkbox
							checked={activeMetrics.includes(metric.key)}
							onCheckedChange={() => handleMetricToggle(metric.key)}
						/>
						<span>{metric.label}</span>
					</label>
				{/each}
			</div>
		{/if}
	{/snippet}

	<ChartPlot
		name={config.chartLabel}
		chartId={config.chartId}
		{series}
		options={chartOptions}
		formatTooltip={(points) =>
			props.kind === 'spectrum'
				? [
						points[0]?.datum.label ?? '',
						`alpha: ${points[0]?.datum.y?.toFixed(6)}`,
						`f(alpha): ${points[0]?.datum.f?.toFixed(6)}`
					].join('\n')
				: [
						points[0]?.datum.label ?? '',
						...points.map(
							(point) =>
								`${point.groupLabel}: ${config.fitYAxisToData ? point.datum.y?.toFixed(3) : point.datum.y?.toLocaleString()}`
						)
					].join('\n')}
		onSelect={(point) =>
			openTemporalPoint(point, IP_TO_GROUP_BY[currentGranularity], drilldown, file)}
		onRange={(start, end) =>
			openTemporalRange(start, end, IP_TO_GROUP_BY[currentGranularity], drilldown)}
	/>
</ChartCard>
