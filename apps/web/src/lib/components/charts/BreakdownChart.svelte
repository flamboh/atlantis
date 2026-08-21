<script lang="ts" generics="Kind extends BreakdownChartKind">
	import { onDestroy, tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { Chart } from './chart-registry';
	import { getRelativePosition } from 'chart.js/helpers';
	import type { ActiveElement, ChartEvent } from 'chart.js';
	import type { GroupByOption, RouterConfig } from '$lib/components/netflow/types.ts';
	import ChartCard from './ChartCard.svelte';
	import { Checkbox } from '$lib/components/ui/checkbox';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { navigateToNetflowFile } from '$lib/utils/netflow-file-navigation';
	import type {
		BucketCoverage,
		FlowVisibility,
		IpGranularity,
		IpMetricKey,
		ProtocolMetricKey,
		SpectrumPoint,
		TimeBucket
	} from '$lib/types/types';
	import type { SpectrumStatsPayload } from '$lib/types/spectrum-stats';
	import {
		BREAKDOWN_CHART_CONFIGS,
		readLineMetric,
		type BreakdownChartKind,
		type BreakdownChartConfig,
		type BreakdownMetricKey,
		type LineBucketData
	} from './breakdown-chart-config';
	import {
		generateSlugFromLabel,
		parseClickedLabel,
		formatNumber,
		Y_AXIS_WIDTH,
		MIN_DRAG_PIXELS,
		groupByBucketDurationMs,
		chooseAdaptiveGranularity,
		createRangeDragState,
		getSelectionLabels,
		indexFromPixelX,
		beginRangeDrag,
		updateRangeDrag,
		endRangeDrag,
		buildMirroredSelectionStyle,
		findTemporalDataBounds,
		getChartBucketCoverage,
		getCoveragePointStyle,
		getCoverageTooltipLines,
		isCoverageSegmentDashed,
		type ChartCoverage
	} from './chart-utils';
	import {
		formatIpGranularityTick,
		formatTemporalBucketLabel,
		shouldHighlightIpGranularityGrid
	} from './ip-time-axis';
	import { dateStringToEpochPST, formatDateAsPSTDateString } from '$lib/utils/timezone';
	import { crosshairStore } from '$lib/stores/crosshair';
	import { rangeSelection } from '$lib/stores/rangeSelection.svelte';
	import { theme } from '$lib/stores/theme.svelte';
	import { cancelDrawFrame, requestDrawFrame } from '$lib/utils/animation-frame';
	import {
		ensureCachedWindow,
		getMissingWindowRanges,
		readCachedWindow,
		type TimeRange
	} from '$lib/utils/window-cache';

	const IP_TO_GROUP_BY: Record<IpGranularity, GroupByOption> = {
		'1d': 'date',
		'1h': 'hour',
		'30m': '30min',
		'5m': '5min'
	};

	const GROUP_BY_TRANSITIONS: Record<GroupByOption, GroupByOption | null> = {
		date: 'hour',
		hour: '30min',
		'30min': '5min',
		'5min': null
	};

	type MetricsForKind<ChartKind extends BreakdownChartKind> = ChartKind extends 'ip'
		? IpMetricKey[]
		: ChartKind extends 'protocol'
			? ProtocolMetricKey[]
			: never[];

	const props = $props<{
		kind: Kind;
		dataset?: string;
		startDate?: string;
		endDate?: string;
		granularity?: IpGranularity;
		router?: string;
		addressType?: 'sa' | 'da';
		availableRouters?: string[];
		routers?: RouterConfig;
		activeMetrics?: MetricsForKind<Kind>;
		srcVisibility?: FlowVisibility;
		dstVisibility?: FlowVisibility;
		onDateChange?: (payload: { startDate: string; endDate: string }) => void;
		onGroupByChange?: (payload: { groupBy: GroupByOption }) => void;
		onRouterChange?: (payload: { router: string }) => void;
		onAddressTypeChange?: (payload: { addressType: 'sa' | 'da' }) => void;
		onMetricsChange?: (payload: { metrics: MetricsForKind<Kind> }) => void;
	}>();
	function getConfig(kind: BreakdownChartKind): BreakdownChartConfig {
		return BREAKDOWN_CHART_CONFIGS[kind];
	}

	const config = $derived(getConfig(props.kind));
	const CHART_ID = $derived(config.chartId);

	const today = new Date();
	const formatDate = (date: Date): string => formatDateAsPSTDateString(date);
	const getInitialAddressType = () => props.addressType ?? 'sa';
	const getInitialRouter = () => (props.router ?? '').trim();
	const getInitialGranularity = () => props.granularity ?? config.defaultGranularity;
	const getInitialMetrics = () => props.activeMetrics ?? config.defaultMetrics;

	type BreakdownBucketData = LineBucketData | SpectrumStatsPayload;
	type BreakdownChartBucket = TimeBucket<BreakdownBucketData>;
	type CachedBreakdownBucket = {
		router: string;
		bucket: BreakdownChartBucket;
	};

	let currentRouter = $state(getInitialRouter());
	let cachedBuckets = $state<CachedBreakdownBucket[]>([]);
	let buckets = $derived(
		cachedBuckets
			.filter((record) => props.kind !== 'spectrum' || record.router === currentRouter)
			.map((record) => record.bucket)
	);
	let activeMetrics = $state<BreakdownMetricKey[]>([...getInitialMetrics()]);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let addressType = $state<'sa' | 'da'>(getInitialAddressType());
	let bucketStarts: number[] = [];

	let chartCanvas = $state<HTMLCanvasElement | null>(null);
	let chart: Chart | null = null;
	let rangeDrag = $state(createRangeDragState());
	let selectionLeft = $derived(Math.min(rangeDrag.dragStartX, rangeDrag.dragCurrentX));
	let selectionWidth = $derived(Math.abs(rangeDrag.dragStartX - rangeDrag.dragCurrentX));
	let mirroredRange = $derived(rangeSelection.selection);
	let pointerMoveFrame: number | null = null;
	let pendingPointerMoveEvent: MouseEvent | null = null;
	let localHoverLabel = $state<string | null>(null);
	let externalHoverLabel = $state<string | null>(null);
	let localHoverX = $state<number | null>(null);
	let externalHoverX = $state<number | null>(null);
	let activeCrosshairX = $derived(localHoverX ?? externalHoverX);
	let showLocalTooltip = $state(false);
	let tooltipTimeout: ReturnType<typeof setTimeout> | null = null;

	function pointsForBucket(bucket: BreakdownChartBucket): SpectrumPoint[] {
		if (!bucket.data || !('spectrumSa' in bucket.data)) return [];
		return addressType === 'sa' ? bucket.data.spectrumSa : bucket.data.spectrumDa;
	}

	function nearestBucketIndex(value: number): number | null {
		if (!Number.isFinite(value) || bucketStarts.length === 0) return null;
		let closestIndex = 0;
		let closestDistance = Math.abs(bucketStarts[0] - value);
		for (let index = 1; index < bucketStarts.length; index += 1) {
			const distance = Math.abs((bucketStarts[index] ?? 0) - value);
			if (distance < closestDistance) {
				closestIndex = index;
				closestDistance = distance;
			}
		}
		return closestIndex;
	}

	const hasSelectedSpectrumData = $derived(
		buckets.some((bucket) => {
			const points = pointsForBucket(bucket);
			return points.length > 0;
		})
	);

	$effect(() => {
		const unsubscribe = crosshairStore.subscribe(({ label, sourceChartId }) => {
			if (sourceChartId === CHART_ID) {
				externalHoverLabel = null;
				externalHoverX = null;
				return;
			}
			externalHoverLabel = label;
			externalHoverX = getPixelForLabel(label);
		});
		return unsubscribe;
	});

	function toEpochSeconds(dateString: string, isEnd = false): number {
		return dateStringToEpochPST(dateString, isEnd);
	}

	function getBucketStartForTickValue(value: unknown): number | null {
		if (typeof value !== 'number' || !Number.isFinite(value)) {
			return null;
		}
		const index = nearestBucketIndex(value);
		return index === null ? null : (bucketStarts[index] ?? null);
	}

	function getChartColors() {
		const style = getComputedStyle(document.documentElement);
		return {
			textColor: style.getPropertyValue('--chart-text-color').trim(),
			gridColor: style.getPropertyValue('--chart-grid-color').trim(),
			gridHighlightColor: style.getPropertyValue('--chart-grid-highlight-color').trim(),
			tooltipBackgroundColor: style.getPropertyValue('--chart-tooltip-bg').trim(),
			tooltipTextColor: style.getPropertyValue('--chart-tooltip-text-color').trim(),
			tooltipBorderColor: style.getPropertyValue('--chart-tooltip-border-color').trim()
		};
	}

	function applyChartTheme() {
		if (!chart) {
			return;
		}

		const {
			textColor,
			gridColor,
			gridHighlightColor,
			tooltipBackgroundColor,
			tooltipTextColor,
			tooltipBorderColor
		} = getChartColors();
		type ThemeableScale = {
			title?: Record<string, unknown>;
			ticks?: Record<string, unknown>;
			grid?: Record<string, unknown>;
		};
		const scales = chart.options.scales as { x?: ThemeableScale; y?: ThemeableScale } | undefined;

		if (scales?.x) {
			scales.x.title = { ...scales.x.title, color: textColor };
			scales.x.ticks = { ...scales.x.ticks, color: textColor };
			scales.x.grid = { ...scales.x.grid, color: scales.x.grid?.color ?? gridHighlightColor };
		}

		if (scales?.y) {
			scales.y.title = { ...scales.y.title, color: textColor };
			scales.y.ticks = { ...scales.y.ticks, color: textColor };
			scales.y.grid = { ...scales.y.grid, color: gridColor };
		}

		if (props.kind !== 'spectrum') {
			chart.options.plugins = {
				...chart.options.plugins,
				legend: { position: 'top', labels: { color: textColor } },
				verticalCrosshair: {
					enabled: true,
					line: {
						color: 'rgba(100, 100, 100, 0.8)',
						width: 1,
						dash: [3, 3]
					},
					tooltip: {
						enabled: true,
						delay: 500,
						backgroundColor: tooltipBackgroundColor,
						textColor: tooltipTextColor,
						borderColor: tooltipBorderColor,
						borderWidth: 1,
						borderRadius: 4,
						padding: 8,
						fontSize: 12,
						fontFamily: 'system-ui, sans-serif'
					},
					sync: {
						onHover: (label: string | null) => crosshairStore.setHover(label, CHART_ID),
						getExternalLabel: () => crosshairStore.getExternalLabel(CHART_ID)
					}
				}
			} as Record<string, unknown>;
		}

		chart.update('none');
	}

	function getPixelForLabel(label: string | null): number | null {
		if (!chart || !label || !chart.data.labels) {
			return null;
		}
		const labels = chart.data.labels as string[];
		const index = labels.indexOf(label);
		if (index === -1) {
			return null;
		}
		const bucketStart = bucketStarts[index];
		return bucketStart === undefined ? null : chart.scales.x.getPixelForValue(bucketStart);
	}

	function syncCrosshairPositions() {
		localHoverX = getPixelForLabel(localHoverLabel);
		externalHoverX = getPixelForLabel(externalHoverLabel);
	}

	function clearTooltipDelay() {
		if (tooltipTimeout !== null) {
			clearTimeout(tooltipTimeout);
			tooltipTimeout = null;
		}
	}

	function scheduleTooltip() {
		clearTooltipDelay();
		showLocalTooltip = false;
		if (!localHoverLabel) {
			return;
		}
		tooltipTimeout = setTimeout(() => {
			showLocalTooltip = true;
		}, 500);
	}

	function clearLocalHover() {
		const hadLocalHover = localHoverLabel !== null || localHoverX !== null;
		localHoverLabel = null;
		localHoverX = null;
		showLocalTooltip = false;
		clearTooltipDelay();
		if (hadLocalHover && crosshairStore.sourceChartId === CHART_ID) {
			crosshairStore.clearHover();
		}
	}

	function hideCrosshairOverlay() {
		clearLocalHover();
		externalHoverX = getPixelForLabel(externalHoverLabel);
	}

	function updateLocalCrosshair(event: MouseEvent) {
		if (!chart || !chartCanvas || rangeDrag.isDraggingRange) {
			return;
		}

		const rect = chartCanvas.getBoundingClientRect();
		const x = event.clientX - rect.left;
		const y = event.clientY - rect.top;
		const area = chart.chartArea;
		const isInChartArea = x >= area.left && x <= area.right && y >= area.top && y <= area.bottom;

		if (!isInChartArea) {
			clearLocalHover();
			return;
		}

		const rawIndex = chart.scales.x.getValueForPixel(x);
		if (typeof rawIndex !== 'number' || !Number.isFinite(rawIndex) || bucketStarts.length === 0) {
			clearLocalHover();
			return;
		}

		const nearestIndex = nearestBucketIndex(rawIndex);
		if (nearestIndex === null) {
			clearLocalHover();
			return;
		}
		const nextIndex = nearestIndex;
		const nextLabel = getLabelFromIndex(nextIndex);
		if (!nextLabel) {
			clearLocalHover();
			return;
		}

		const nextX = chart.scales.x.getPixelForValue(bucketStarts[nextIndex] ?? 0);
		const labelChanged = nextLabel !== localHoverLabel;
		localHoverLabel = nextLabel;
		localHoverX = nextX;
		if (labelChanged) {
			scheduleTooltip();
			crosshairStore.setHover(nextLabel, CHART_ID);
		}
	}

	function getCrosshairLineStyle(x: number | null): string | null {
		if (x === null || !chart) {
			return null;
		}
		const area = chart.chartArea;
		const snappedX = Math.round(x) + 0.5;
		return `left:${snappedX}px; top:${area.top}px; width:1px; height:${area.bottom - area.top}px; background-image:repeating-linear-gradient(to bottom, rgba(100,100,100,0.8) 0 3px, transparent 3px 6px);`;
	}

	function getCrosshairTooltipStyle(x: number | null): string | null {
		if (x === null || !chart) {
			return null;
		}

		const area = chart.chartArea;
		const snappedX = Math.round(x) + 0.5;
		const tooltipWidth = 190;
		const left = Math.min(
			Math.max(snappedX - tooltipWidth / 2, area.left + 5),
			area.right - tooltipWidth - 5
		);
		const top = Math.max(6, area.top - 34);
		return `left:${left}px; top:${top}px; width:${tooltipWidth}px;`;
	}

	// Color gradient function based on f value
	// Purple (low f) -> Blue -> Cyan -> Green -> Yellow (high f)
	function getColorForF(f: number, minF: number, maxF: number): string {
		if (maxF === minF) return 'hsl(180, 70%, 50%)';
		const normalized = (f - minF) / (maxF - minF);
		// HSL: hue 270=purple, 180=cyan, 120=green, 60=yellow
		const hue = 270 - normalized * 210; // 270 (purple) to 60 (yellow)
		return `hsl(${hue}, 70%, 50%)`;
	}

	function destroyChart() {
		if (chart) {
			if (props.kind !== 'spectrum') {
				crosshairStore.unregister(CHART_ID);
			}
			chart.destroy();
			chart = null;
		}
		if (crosshairStore.sourceChartId === CHART_ID) {
			crosshairStore.clearHover();
		}
		clearTooltipDelay();
		localHoverLabel = null;
		localHoverX = null;
		showLocalTooltip = false;
		externalHoverX = null;
	}

	function deriveSelectedRouters(routerConfig: RouterConfig | undefined): string[] {
		if (!routerConfig) {
			return [];
		}
		return Object.entries(routerConfig)
			.filter(([, enabled]) => enabled)
			.map(([name]) => name.trim())
			.filter((name) => name.length > 0)
			.sort();
	}

	function buildColors(metricIndex: number, routerIndex: number) {
		const metric = config.metrics[metricIndex];
		if (!metric) {
			return { stroke: 'transparent', fill: 'transparent' };
		}
		const hue = (metric.color.hue + routerIndex * config.routerHueStep) % 360;
		const stroke = `hsl(${hue}, ${metric.color.saturation}%, ${metric.color.lightness}%)`;
		const fill = `hsla(${hue}, ${metric.color.saturation}%, ${metric.color.lightness}%, ${config.fillAlpha})`;
		return { stroke, fill };
	}

	function handleMetricToggle(metric: BreakdownMetricKey) {
		const nextMetrics = activeMetrics.includes(metric)
			? activeMetrics.filter((item) => item !== metric)
			: [...activeMetrics, metric];
		activeMetrics = nextMetrics;
		props.onMetricsChange?.({
			metrics: nextMetrics as MetricsForKind<Kind>
		});
	}

	function emitDrilldown(nextGroupBy: GroupByOption, start: Date, end: Date) {
		props.onGroupByChange?.({ groupBy: nextGroupBy });
		props.onDateChange?.({
			startDate: formatDate(start),
			endDate: formatDate(end)
		});
	}

	function handleRouterChange(router: string) {
		if (router === (props.router ?? '')) {
			return;
		}
		props.onRouterChange?.({ router });
	}

	function handleAddressTypeChange(nextAddressType: 'sa' | 'da') {
		if (nextAddressType === addressType) {
			return;
		}
		addressType = nextAddressType;
		if (chart) {
			renderChart();
		}
		props.onAddressTypeChange?.({ addressType: nextAddressType });
	}

	function publishRangeSelection(startIndex: number, endIndex: number) {
		const labels = getSelectionLabels(chart, startIndex, endIndex);
		if (!labels) return;
		rangeSelection.set({ sourceChartId: CHART_ID, ...labels });
	}

	function applyRangeDrilldown(startIndex: number, endIndex: number) {
		if (!chart?.data.labels) return;
		const labels = chart.data.labels as string[];
		const from = Math.max(0, Math.min(labels.length - 1, Math.min(startIndex, endIndex)));
		const to = Math.max(0, Math.min(labels.length - 1, Math.max(startIndex, endIndex)));
		const startLabel = labels[from];
		const endLabel = labels[to];
		if (!startLabel || !endLabel) return;

		const groupBy = IP_TO_GROUP_BY[currentGranularity];
		if (!groupBy) return;

		const startDate = parseClickedLabel(startLabel, groupBy);
		const endBucketStart = parseClickedLabel(endLabel, groupBy);
		if (Number.isNaN(startDate.getTime()) || Number.isNaN(endBucketStart.getTime())) return;
		const endExclusive = new Date(endBucketStart.getTime() + groupByBucketDurationMs(groupBy));
		const selectedRangeMs = endExclusive.getTime() - startDate.getTime();
		const nextGroupBy = chooseAdaptiveGranularity(selectedRangeMs);
		emitDrilldown(nextGroupBy, startDate, endExclusive);
	}

	function handleRangeMouseDown(event: MouseEvent) {
		cancelPendingPointerMove();
		if (props.kind === 'spectrum') {
			clearLocalHover();
		}
		beginRangeDrag(rangeDrag, event, chartCanvas, chart, publishRangeSelection);
	}

	function applyPendingPointerMove() {
		pointerMoveFrame = null;
		const event = pendingPointerMoveEvent;
		pendingPointerMoveEvent = null;
		if (!event) {
			return;
		}
		updateRangeDrag(rangeDrag, event, chartCanvas, chart, publishRangeSelection);
		if (props.kind === 'spectrum') {
			updateLocalCrosshair(event);
		}
	}

	function cancelPendingPointerMove() {
		if (pointerMoveFrame !== null) {
			cancelDrawFrame(pointerMoveFrame);
			pointerMoveFrame = null;
		}
		pendingPointerMoveEvent = null;
	}

	function flushPendingPointerMove() {
		if (pointerMoveFrame === null) {
			return;
		}
		cancelDrawFrame(pointerMoveFrame);
		applyPendingPointerMove();
	}

	function handleRangeMouseMove(event: MouseEvent) {
		if (!rangeDrag.isDraggingRange && props.kind !== 'spectrum') {
			return;
		}
		pendingPointerMoveEvent = event;
		if (pointerMoveFrame === null) {
			pointerMoveFrame = requestDrawFrame(applyPendingPointerMove);
		}
	}

	function finishRangeSelection() {
		flushPendingPointerMove();
		endRangeDrag(rangeDrag, chart, applyRangeDrilldown);
		rangeSelection.clear();
	}

	function handlePointerLeave() {
		finishRangeSelection();
		hideCrosshairOverlay();
	}

	let mirroredSelectionStyle = $derived(
		buildMirroredSelectionStyle(chart, mirroredRange, CHART_ID)
	);

	function getLabelFromIndex(index: number): string | null {
		if (!chart || !chart.data.labels) {
			return null;
		}
		const labels = chart.data.labels as string[];
		if (index < 0 || index >= labels.length) {
			return null;
		}
		return labels[index] ?? null;
	}

	function handleChartClick(event: ChartEvent, activeElements: ActiveElement[]) {
		if (rangeDrag.suppressNextClick) {
			rangeDrag.suppressNextClick = false;
			return;
		}
		if (!chart || !chart.data.labels) {
			return;
		}

		const groupBy = IP_TO_GROUP_BY[currentGranularity];
		if (!groupBy) {
			return;
		}

		const labels = chart.data.labels as string[];
		if (labels.length === 0 || bucketStarts.length === 0) {
			return;
		}

		const canvasPosition = getRelativePosition(event, chart);
		const dataX = chart.scales.x.getValueForPixel(canvasPosition.x);
		const labelIndex =
			props.kind === 'spectrum'
				? typeof dataX === 'number' && Number.isFinite(dataX)
					? nearestBucketIndex(dataX)
					: null
				: indexFromPixelX(chart, canvasPosition.x);
		const fallbackIndex =
			activeElements.length === 0
				? null
				: props.kind === 'spectrum'
					? nearestBucketIndex(
							bucketStarts[Math.min(activeElements[0]?.index ?? 0, bucketStarts.length - 1)] ?? 0
						)
					: activeElements[0].index;
		const targetIndex = labelIndex ?? fallbackIndex;
		const label = targetIndex !== null ? getLabelFromIndex(targetIndex) : labels[0];

		if (!label) {
			return;
		}

		const clickedDate = parseClickedLabel(label, groupBy);
		if (!(clickedDate instanceof Date) || Number.isNaN(clickedDate.getTime())) {
			if (props.kind !== 'protocol') {
				console.warn('Unable to parse clicked label for drilldown', { label, groupBy });
			}
			return;
		}
		const activeLabel = fallbackIndex !== null ? getLabelFromIndex(fallbackIndex) : null;

		if (groupBy === '5min') {
			const labelForSlug = activeLabel ?? label;
			const slug = generateSlugFromLabel(labelForSlug, '5min');
			if (slug) {
				void navigateToNetflowFile(goto, slug, props.dataset, {
					srcVisibility: props.srcVisibility ?? 'all',
					dstVisibility: props.dstVisibility ?? 'all'
				});
			}
			return;
		}

		const nextGroupBy = GROUP_BY_TRANSITIONS[groupBy];
		if (!nextGroupBy) {
			return;
		}

		if (groupBy === 'date') {
			const rangeStart = new Date(clickedDate.getTime() - 15 * 24 * 60 * 60 * 1000);
			const rangeEnd = new Date(clickedDate.getTime() + 16 * 24 * 60 * 60 * 1000);
			emitDrilldown(nextGroupBy, rangeStart, rangeEnd);
		} else if (groupBy === 'hour') {
			const rangeStart = new Date(clickedDate.getTime() - 3 * 24 * 60 * 60 * 1000);
			const rangeEnd = new Date(clickedDate.getTime() + 4 * 24 * 60 * 60 * 1000);
			emitDrilldown(nextGroupBy, rangeStart, rangeEnd);
		} else if (groupBy === '30min') {
			const rangeEnd = new Date(clickedDate.getTime() + 24 * 60 * 60 * 1000);
			emitDrilldown(nextGroupBy, clickedDate, rangeEnd);
		}
	}

	interface DataPoint {
		x: number;
		y: number;
		f: number;
		timeLabel: string;
	}

	function buildDatasets(
		selectedBuckets: BreakdownChartBucket[],
		bucketStarts: number[]
	): {
		data: DataPoint[];
		minF: number;
		maxF: number;
		minAlpha: number;
		maxAlpha: number;
	} {
		const pointsByBucketStart: Record<number, SpectrumPoint[]> = {};
		selectedBuckets.forEach((bucket) => {
			const points = pointsForBucket(bucket);
			if (points.length > 0) {
				pointsByBucketStart[bucket.bucketStart] = points;
			}
		});

		// Find global min/max for f and alpha
		let minF = Infinity;
		let maxF = -Infinity;
		let minAlpha = Infinity;
		let maxAlpha = -Infinity;

		Object.values(pointsByBucketStart).forEach((points) => {
			points.forEach((point) => {
				minF = Math.min(minF, point.f);
				maxF = Math.max(maxF, point.f);
				minAlpha = Math.min(minAlpha, point.alpha);
				maxAlpha = Math.max(maxAlpha, point.alpha);
			});
		});

		if (minF === Infinity) {
			return { data: [], minF: 0, maxF: 0, minAlpha: 0, maxAlpha: 0 };
		}

		// Build data points: each (time, alpha) has an f value for coloring
		const data: DataPoint[] = [];

		bucketStarts.forEach((bucketStart) => {
			const points = pointsByBucketStart[bucketStart];
			if (!points || points.length === 0) return;

			const timeLabel = formatTemporalBucketLabel(bucketStart, currentGranularity);

			points.forEach((point) => {
				data.push({
					x: bucketStart,
					y: point.alpha,
					f: point.f,
					timeLabel
				});
			});
		});

		return { data, minF, maxF, minAlpha, maxAlpha };
	}

	function buildLineScales(textColor: string, gridColor: string, gridHighlightColor: string) {
		return {
			x: {
				type: 'linear' as const,
				min: bucketStarts[0],
				max: bucketStarts[bucketStarts.length - 1],
				title: { display: true, text: `Time (${currentGranularity})`, color: textColor },
				ticks: {
					color: textColor,
					autoSkip: false,
					maxRotation: 45,
					minRotation: 45,
					sampleSize: 12,
					callback: (value: string | number) =>
						formatIpGranularityTick(Number(value), currentGranularity, 0)
				},
				grid: {
					color: (context: { tick?: { value?: number } }) =>
						shouldHighlightIpGranularityGrid(
							Number(context.tick?.value ?? 0),
							currentGranularity,
							0
						)
							? gridColor
							: gridHighlightColor
				}
			},
			y: {
				beginAtZero: true,
				afterFit(axis: { width: number }) {
					axis.width = Y_AXIS_WIDTH;
				},
				title: { display: true, text: config.yAxisTitle, color: textColor },
				ticks: config.formatYAxisTicks
					? {
							color: textColor,
							callback: (value: string | number) => formatNumber(Number(value))
						}
					: { color: textColor },
				grid: { color: gridColor }
			}
		};
	}

	function buildLinePlugins(
		textColor: string,
		tooltipBackgroundColor: string,
		tooltipTextColor: string,
		tooltipBorderColor: string
	) {
		return {
			legend: { position: 'top', labels: { color: textColor } },
			tooltip: {
				callbacks: {
					afterLabel: (context: { raw: unknown }) =>
						getCoverageTooltipLines(getChartBucketCoverage(context.raw))
				}
			},
			verticalCrosshair: {
				enabled: true,
				line: {
					color: 'rgba(100, 100, 100, 0.8)',
					width: 1,
					dash: [3, 3]
				},
				tooltip: {
					enabled: true,
					delay: 500,
					backgroundColor: tooltipBackgroundColor,
					textColor: tooltipTextColor,
					borderColor: tooltipBorderColor,
					borderWidth: 1,
					borderRadius: 4,
					padding: 8,
					fontSize: 12,
					fontFamily: 'system-ui, sans-serif'
				},
				sync: {
					onHover: (label: string | null) => crosshairStore.setHover(label, CHART_ID),
					getExternalLabel: () => crosshairStore.getExternalLabel(CHART_ID)
				}
			}
		} as Record<string, unknown>;
	}

	function renderLineChart() {
		const {
			textColor,
			gridColor,
			gridHighlightColor,
			tooltipBackgroundColor,
			tooltipTextColor,
			tooltipBorderColor
		} = getChartColors();
		const selectedRouters = new Set(deriveSelectedRouters(props.routers));
		const selectedBuckets = cachedBuckets.filter((record) => selectedRouters.has(record.router));

		if (activeMetrics.length === 0 || selectedBuckets.length === 0) {
			destroyChart();
			return;
		}

		const canvas = chartCanvas;
		if (!canvas) return;

		bucketStarts = Array.from(
			new Set(selectedBuckets.map((record) => record.bucket.bucketStart))
		).sort((left, right) => left - right);
		const routers = Array.from(new Set(selectedBuckets.map((record) => record.router))).sort();
		const labels = bucketStarts.map((bucketStart) =>
			formatTemporalBucketLabel(bucketStart, currentGranularity)
		);
		const bucketByRouterAndStart = new Map(
			selectedBuckets.map((record) => [`${record.router}-${record.bucket.bucketStart}`, record])
		);

		const datasets = routers.flatMap((router, routerIndex) =>
			config.metrics
				.filter((metric) => activeMetrics.includes(metric.key))
				.map((metric) => {
					const configIndex = config.metrics.findIndex((candidate) => candidate.key === metric.key);
					const { stroke, fill } = buildColors(configIndex, routerIndex);
					const data = bucketStarts.map((bucketStart) => {
						const record = bucketByRouterAndStart.get(`${router}-${bucketStart}`);
						const bucket = record?.bucket;
						const coverage = getChartBucketCoverage(bucket) ?? {
							state: 'unknown',
							observedUnits: 0,
							expectedUnits: 0
						};
						return {
							x: bucketStart,
							y: readLineMetric((bucket?.data as LineBucketData | null) ?? null, metric.key),
							bucketStart,
							bucketEnd: bucket?.bucketEnd ?? bucketStart,
							coverage
						};
					});
					const pointStyles = data.map((point) => getCoveragePointStyle(point.coverage, stroke));
					return {
						label: `${router} · ${metric.seriesLabel}`,
						data,
						borderColor: stroke,
						backgroundColor: fill,
						tension: 0.3,
						fill: false,
						pointRadius: data.map((point, index) =>
							point.y === null ? 0 : (pointStyles[index]?.radius ?? 0)
						),
						pointBackgroundColor: pointStyles.map((style) => style.backgroundColor),
						pointBorderColor: pointStyles.map((style) => style.borderColor),
						pointBorderWidth: pointStyles.map((style) => style.borderWidth),
						pointHoverRadius: 4,
						spanGaps: false,
						segment: {
							borderDash: (context: { p0: { raw: unknown }; p1: { raw: unknown } }) =>
								isCoverageSegmentDashed(
									context.p0.raw as { coverage?: ChartCoverage },
									context.p1.raw as { coverage?: ChartCoverage }
								)
									? [6, 4]
									: []
						},
						parsing: false
					};
				})
		);

		if (datasets.length === 0 || labels.length === 0) {
			if (chart) {
				chart.data.labels = [];
				chart.data.datasets = [];
				chart.update();
			}
			return;
		}

		const scales = buildLineScales(textColor, gridColor, gridHighlightColor);
		const plugins = buildLinePlugins(
			textColor,
			tooltipBackgroundColor,
			tooltipTextColor,
			tooltipBorderColor
		);
		if (!chart) {
			chart = new Chart(canvas, {
				type: 'line',
				data: { labels, datasets },
				options: {
					onClick: handleChartClick,
					responsive: true,
					maintainAspectRatio: false,
					animation: false,
					interaction: { mode: 'index', intersect: false },
					plugins,
					scales
				}
			} as never);
			crosshairStore.register(CHART_ID, chart);
		} else {
			chart.data.labels = labels;
			chart.data.datasets = datasets as never[];
			chart.options.scales = scales as never;
			chart.options.onClick = handleChartClick;
			chart.options.plugins = plugins;
			chart.update('none');
		}
	}

	function renderSpectrumChart() {
		const { textColor, gridColor, gridHighlightColor } = getChartColors();
		const canvas = chartCanvas;
		if (!canvas) {
			return;
		}

		const selectedBuckets = currentRouter ? buckets : [];

		// Get unique time buckets, sorted
		bucketStarts = Array.from(new Set(selectedBuckets.map((b) => b.bucketStart))).sort(
			(a, b) => a - b
		);

		if (bucketStarts.length === 0) {
			if (chart) {
				chart.data.datasets = [];
				chart.update('none');
			}
			syncCrosshairPositions();
			return;
		}

		const { data, minF, maxF, minAlpha, maxAlpha } = buildDatasets(selectedBuckets, bucketStarts);

		if (data.length === 0) {
			if (chart) {
				chart.data.datasets = [];
				chart.update('none');
			}
			syncCrosshairPositions();
			return;
		}
		const dataBounds = findTemporalDataBounds(
			data,
			(point) => point.x,
			() => true
		);
		if (!dataBounds) return;

		const labels = bucketStarts.map((bucketStart) =>
			formatTemporalBucketLabel(bucketStart, currentGranularity)
		);

		// Create scatter dataset with individual point colors based on f
		const pointColors = data.map((d) => getColorForF(d.f, minF, maxF));
		const chartPoints = data.map((d) => ({
			x: d.x,
			y: d.y
		}));

		const granularity = currentGranularity;
		const alphaPadding = (maxAlpha - minAlpha) * 0.05;

		if (!chart) {
			chart = new Chart(canvas, {
				type: 'scatter',
				data: {
					labels,
					datasets: [
						{
							data: chartPoints,
							backgroundColor: pointColors,
							borderColor: pointColors,
							pointRadius: 1,
							pointHoverRadius: 2
						}
					]
				},
				options: {
					onClick: handleChartClick,
					animation: false as const,
					responsive: true,
					maintainAspectRatio: false,
					events: ['click'],
					interaction: {
						mode: 'nearest',
						intersect: true
					},
					plugins: {
						legend: {
							display: false
						},
						tooltip: {
							enabled: false
						}
					} as Record<string, unknown>,
					scales: {
						x: {
							type: 'linear',
							min: dataBounds.min,
							max: dataBounds.max,
							title: {
								display: true,
								text: `Time (${granularity})`,
								color: textColor
							},
							ticks: {
								color: textColor,
								autoSkip: false,
								maxRotation: 45,
								minRotation: 45,
								sampleSize: 12,
								callback: (value: unknown) => {
									const bucketStart = getBucketStartForTickValue(value);
									if (bucketStart === null) return '';
									const index = typeof value === 'number' ? (nearestBucketIndex(value) ?? 0) : 0;
									return formatIpGranularityTick(bucketStart, granularity, index);
								}
							},
							grid: {
								color: (ctx: { tick?: { value?: number } }) => {
									const tickValue = ctx.tick?.value;
									const bucketStart = getBucketStartForTickValue(tickValue);
									if (bucketStart === null || typeof tickValue !== 'number') {
										return gridHighlightColor;
									}
									const index = nearestBucketIndex(tickValue) ?? 0;
									return shouldHighlightIpGranularityGrid(bucketStart, granularity, index)
										? gridColor
										: gridHighlightColor;
								}
							}
						},
						y: {
							type: 'linear',
							min: minAlpha - alphaPadding,
							max: maxAlpha + alphaPadding,
							afterFit(axis: { width: number }) {
								axis.width = Y_AXIS_WIDTH;
							},
							title: {
								display: true,
								text: 'alpha',
								color: textColor
							},
							ticks: { color: textColor },
							grid: { color: gridColor }
						}
					}
				}
			});
		} else {
			chart.data.labels = labels;
			chart.data.datasets = [
				{
					data: chartPoints,
					backgroundColor: pointColors,
					borderColor: pointColors,
					pointRadius: 1,
					pointHoverRadius: 2
				}
			];
			chart.options.scales = {
				x: {
					type: 'linear',
					min: dataBounds.min,
					max: dataBounds.max,
					title: { display: true, text: `Time (${granularity})`, color: textColor },
					ticks: {
						color: textColor,
						autoSkip: false,
						maxRotation: 45,
						minRotation: 45,
						sampleSize: 12,
						callback: (value: unknown) => {
							const bucketStart = getBucketStartForTickValue(value);
							if (bucketStart === null) return '';
							const index = typeof value === 'number' ? (nearestBucketIndex(value) ?? 0) : 0;
							return formatIpGranularityTick(bucketStart, granularity, index);
						}
					},
					grid: {
						color: (ctx: { tick?: { value?: number } }) => {
							const tickValue = ctx.tick?.value;
							const bucketStart = getBucketStartForTickValue(tickValue);
							if (bucketStart === null || typeof tickValue !== 'number') {
								return gridHighlightColor;
							}
							const index = nearestBucketIndex(tickValue) ?? 0;
							return shouldHighlightIpGranularityGrid(bucketStart, granularity, index)
								? gridColor
								: gridHighlightColor;
						}
					}
				},
				y: {
					min: minAlpha - alphaPadding,
					max: maxAlpha + alphaPadding,
					afterFit(axis: { width: number }) {
						axis.width = Y_AXIS_WIDTH;
					},
					title: { display: true, text: 'alpha', color: textColor },
					ticks: { color: textColor },
					grid: { color: gridColor }
				}
			};
			chart.options.onClick = handleChartClick;
			chart.update('none');
		}
		syncCrosshairPositions();
	}

	function renderChart() {
		if (props.kind === 'spectrum') {
			renderSpectrumChart();
		} else {
			renderLineChart();
		}
	}

	type FilterInputs = {
		startDate: string;
		endDate: string;
		granularity: IpGranularity;
		routers: string[];
		srcVisibility: FlowVisibility;
		dstVisibility: FlowVisibility;
	};

	let lastFiltersKey = '';
	let lastIncomingMetricsKey = '';
	let requestToken = 0;

	function getRequestedRange(filters: FilterInputs): TimeRange {
		return {
			start: toEpochSeconds(filters.startDate),
			end: toEpochSeconds(filters.endDate, true)
		};
	}

	function getCacheKey(filters: FilterInputs): string {
		return JSON.stringify({
			chart: CHART_ID,
			dataset: props.dataset ?? '',
			granularity: filters.granularity,
			routers: filters.routers,
			srcVisibility: filters.srcVisibility,
			dstVisibility: filters.dstVisibility
		});
	}

	async function loadData(filters: FilterInputs, token: number) {
		const requestedRange = getRequestedRange(filters);
		const cacheKey = getCacheKey(filters);
		loading = getMissingWindowRanges(cacheKey, requestedRange).length > 0;
		error = null;
		if (loading) {
			destroyChart();
		}

		const params = new URLSearchParams({
			dataset: props.dataset ?? '',
			granularity: filters.granularity,
			routers: filters.routers.join(','),
			srcVisibility: filters.srcVisibility,
			dstVisibility: filters.dstVisibility
		});

		try {
			await ensureCachedWindow<CachedBreakdownBucket>({
				key: cacheKey,
				requestedRange,
				fetchRange: async (range) => {
					const response = await fetch(
						`${config.endpoint}?${new URLSearchParams({
							...Object.fromEntries(params.entries()),
							startDate: range.start.toString(),
							endDate: range.end.toString()
						}).toString()}`
					);
					if (!response.ok) {
						const message = await response.text();
						throw new Error(message || config.fetchErrorCopy);
					}
					const data = (await response.json()) as {
						timelines: Array<{ router: string; buckets: BreakdownChartBucket[] }>;
					};
					return data.timelines.flatMap((timeline) =>
						timeline.buckets.map((bucket) => ({
							router: timeline.router,
							bucket
						}))
					);
				},
				getRecordKey: (record) => `${record.router}-${record.bucket.bucketStart}`,
				compareRecords: (left, right) =>
					left.bucket.bucketStart - right.bucket.bucketStart ||
					left.router.localeCompare(right.router)
			});
			if (token !== requestToken) {
				return;
			}
			cachedBuckets = readCachedWindow<CachedBreakdownBucket>(
				cacheKey,
				requestedRange,
				(record, range) => {
					return record.bucket.bucketStart >= range.start && record.bucket.bucketStart < range.end;
				}
			);
			loading = false;
			await tick();
			renderChart();
		} catch (err) {
			if (token !== requestToken) {
				return;
			}
			error = err instanceof Error ? err.message : config.unexpectedErrorCopy;
			cachedBuckets = [];
			loading = false;
			destroyChart();
		}
	}

	onDestroy(() => {
		cancelPendingPointerMove();
		if (mirroredRange?.sourceChartId === CHART_ID) {
			rangeSelection.clear();
		}
		destroyChart();
	});

	let currentGranularity = $state<IpGranularity>(getInitialGranularity());

	$effect(() => {
		void theme.dark;
		if (chart) {
			applyChartTheme();
		}
	});

	$effect(() => {
		if (props.kind === 'spectrum') {
			return;
		}
		const incomingMetrics = props.activeMetrics ?? config.defaultMetrics;
		const nextKey = JSON.stringify(incomingMetrics);
		if (nextKey === lastIncomingMetricsKey) {
			return;
		}
		lastIncomingMetricsKey = nextKey;
		activeMetrics = [...incomingMetrics];
		void (async () => {
			await tick();
			renderChart();
		})();
	});

	$effect(() => {
		if (props.kind !== 'spectrum') {
			const routerConfig = props.routers;
			if (!routerConfig || Object.keys(routerConfig).length === 0) {
				return;
			}
			const selectedRouters = deriveSelectedRouters(routerConfig);
			const filters: FilterInputs = {
				startDate: props.startDate ?? '2025-01-01',
				endDate: props.endDate ?? formatDate(today),
				granularity: props.granularity ?? config.defaultGranularity,
				routers: selectedRouters,
				srcVisibility: props.srcVisibility ?? 'all',
				dstVisibility: props.dstVisibility ?? 'all'
			};

			currentGranularity = filters.granularity;
			if (selectedRouters.length === 0) {
				error = config.noSourceCopy;
				cachedBuckets = [];
				destroyChart();
				lastFiltersKey = JSON.stringify({ ...filters, selectedRouters });
				loading = false;
				return;
			}

			error = null;
			const nextKey = JSON.stringify({ ...filters, selectedRouters });
			if (nextKey === lastFiltersKey) {
				return;
			}
			lastFiltersKey = nextKey;
			const token = ++requestToken;
			loadData(filters, token);
			return;
		}

		const availableRouters = (props.availableRouters ?? [])
			.map((router: string) => router.trim())
			.filter((router: string) => router.length > 0);
		const requestedRouter = props.router?.trim() ?? '';
		const nextRouter = availableRouters.includes(requestedRouter)
			? requestedRouter
			: (availableRouters[0] ?? '');
		const startDateProp = props.startDate;
		const endDateProp = props.endDate;
		const granularityProp = props.granularity;
		const nextAddressType = props.addressType ?? 'sa';
		const srcVisibility = props.srcVisibility ?? 'all';
		const dstVisibility = props.dstVisibility ?? 'all';

		if (nextAddressType !== addressType) {
			addressType = nextAddressType;
			if (chart) {
				renderChart();
			}
		}
		if (nextRouter !== currentRouter) {
			currentRouter = nextRouter;
			if (chart) {
				renderChart();
			}
		}

		const filters: FilterInputs = {
			startDate: startDateProp ?? '2025-01-01',
			endDate: endDateProp ?? formatDate(today),
			granularity: granularityProp ?? config.defaultGranularity,
			routers: availableRouters,
			srcVisibility,
			dstVisibility
		};

		currentGranularity = filters.granularity;

		if (filters.routers.length === 0) {
			error = config.noSourceCopy;
			cachedBuckets = [];
			destroyChart();
			lastFiltersKey = JSON.stringify(filters);
			loading = false;
			return;
		}

		const nextKey = JSON.stringify(filters);
		if (nextKey === lastFiltersKey) {
			return;
		}

		lastFiltersKey = nextKey;
		const token = ++requestToken;
		loadData(filters, token);
	});
</script>

<ChartCard
	title={config.title}
	size={props.kind === 'spectrum' ? 'spectrum' : 'default'}
	{loading}
	{error}
	noMetrics={props.kind === 'spectrum'
		? buckets.length > 0 && !hasSelectedSpectrumData
		: activeMetrics.length === 0}
	empty={buckets.length === 0}
	loadingCopy={config.loadingCopy}
	noMetricsCopy={props.kind === 'spectrum'
		? `No ${addressType === 'sa' ? 'source' : 'destination'} spectrum data for the selected source.`
		: config.noMetricsCopy}
	emptyCopy={config.emptyCopy}
	isDraggingRange={rangeDrag.isDraggingRange}
	{selectionLeft}
	{selectionWidth}
	selectionTop={rangeDrag.selectionTop}
	selectionHeight={rangeDrag.selectionHeight}
	{mirroredSelectionStyle}
	minDragPixels={MIN_DRAG_PIXELS}
	onmousedown={handleRangeMouseDown}
	onmousemove={handleRangeMouseMove}
	onmouseup={finishRangeSelection}
	onmouseleave={props.kind === 'spectrum' ? handlePointerLeave : finishRangeSelection}
>
	{#snippet controls()}
		{#if props.kind === 'spectrum'}
			<div class="space-y-2">
				<div class="flex min-h-6 flex-wrap items-center gap-4">
					{#if (props.availableRouters ?? []).length === 0}
						{#each Array(4) as _, index (index)}
							<Skeleton class="inline-block h-4 w-24" aria-hidden="true" />
						{/each}
					{:else}
						{#each props.availableRouters ?? [] as routerName (routerName)}
							<label class="text-foreground flex cursor-pointer items-center gap-2 text-sm">
								<input
									type="radio"
									name="spectrum-router-local"
									checked={props.router === routerName}
									onchange={() => handleRouterChange(routerName)}
									class="border-input accent-primary focus-visible:ring-ring size-4 focus-visible:ring-2"
								/>
								<span>{routerName}</span>
							</label>
						{/each}
					{/if}
				</div>
				<div class="flex flex-wrap items-center gap-4">
					{#each [['sa', 'Source IPv4'], ['da', 'Destination IPv4']] as const as addressOption (addressOption[0])}
						<label class="text-foreground flex cursor-pointer items-center gap-2 text-sm">
							<input
								type="radio"
								name="spectrum-address-type-local"
								checked={addressType === addressOption[0]}
								onchange={() => handleAddressTypeChange(addressOption[0])}
								class="border-input accent-primary focus-visible:ring-ring size-4 focus-visible:ring-2"
							/>
							<span>{addressOption[1]}</span>
						</label>
					{/each}
				</div>
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

	<canvas bind:this={chartCanvas} aria-label={config.canvasLabel}></canvas>

	{#snippet overlay()}
		{#if props.kind === 'spectrum'}
			{#if !rangeDrag.isDraggingRange && activeCrosshairX !== null}
				<div
					class="pointer-events-none absolute z-20"
					style={getCrosshairLineStyle(activeCrosshairX)}
				></div>
			{/if}
			{#if !rangeDrag.isDraggingRange && localHoverX !== null && showLocalTooltip && localHoverLabel}
				<div
					class="pointer-events-none absolute z-20 rounded border px-2 py-1 text-xs whitespace-nowrap shadow-sm"
					style={`${getCrosshairTooltipStyle(localHoverX)} background:${getChartColors().tooltipBackgroundColor}; color:${getChartColors().tooltipTextColor}; border-color:${getChartColors().tooltipBorderColor};`}
				>
					<div>{localHoverLabel}</div>
					{#each getCoverageLinesForLabel(localHoverLabel) as line (line)}
						<div>{line}</div>
					{/each}
				</div>
			{/if}
		{/if}
	{/snippet}
</ChartCard>
