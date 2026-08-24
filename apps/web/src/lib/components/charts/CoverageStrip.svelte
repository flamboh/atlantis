<script module lang="ts">
	import type { CoverageTimeline, CoverageTimelineBucket } from '$lib/types/types';
	import { SvelteMap } from 'svelte/reactivity';

	export type CachedCoverageRecord = {
		sourceId: string;
		bucket: CoverageTimelineBucket;
	};

	export function flattenCoverageTimelines(timelines: CoverageTimeline[]): CachedCoverageRecord[] {
		return timelines.flatMap((timeline) =>
			timeline.buckets.map((bucket) => ({ sourceId: timeline.sourceId, bucket }))
		);
	}

	export function rebuildCoverageTimelines(
		records: CachedCoverageRecord[],
		sourceIds: string[]
	): CoverageTimeline[] {
		const bucketsBySource = new SvelteMap(
			sourceIds.map((sourceId) => [sourceId, [] as CoverageTimelineBucket[]])
		);
		for (const record of records) {
			const buckets = bucketsBySource.get(record.sourceId) ?? [];
			buckets.push(record.bucket);
			bucketsBySource.set(record.sourceId, buckets);
		}
		return [...bucketsBySource].map(([sourceId, buckets]) => ({
			sourceId,
			buckets: buckets.sort((left, right) => left.bucketStart - right.bucketStart)
		}));
	}
</script>

<script lang="ts">
	import { onDestroy } from 'svelte';
	import type { Plugin, TooltipItem } from 'chart.js';
	import DragGrip from '$lib/components/common/DragGrip.svelte';
	import type { BucketCoverage, CoverageState } from '$lib/types/types';
	import type { GroupByOption, RouterConfig } from '$lib/components/netflow/types';
	import { dateStringToEpochPST } from '$lib/utils/timezone';
	import {
		ensureCachedWindow,
		getMissingWindowRanges,
		readCachedWindow,
		type TimeRange
	} from '$lib/utils/window-cache';
	import { crosshairStore } from '$lib/stores/crosshair';
	import { theme } from '$lib/stores/theme.svelte';
	import { Chart } from './chart-registry';
	import { findTemporalDataBounds } from './chart-utils';
	import {
		formatCoverageState,
		formatCoverageStripLabel,
		type CoverageStripBucket,
		type CoverageStripTimeline
	} from './coverage-strip';

	const CHART_ID = 'coverage-strip';

	const props = $props<{
		dataset: string;
		startDate: string;
		endDate: string;
		groupBy: GroupByOption;
		routers: RouterConfig;
		routersLoaded: boolean;
	}>();

	type ChartPalette = {
		textColor: string;
		trackColor: string;
		tooltipBackgroundColor: string;
		tooltipTextColor: string;
		tooltipBorderColor: string;
		completeColor: string;
		partialColor: string;
	};

	let timelines = $state.raw<CoverageStripTimeline[]>([]);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let canvas = $state<HTMLCanvasElement | null>(null);
	let chart: Chart<'line', number[], string> | null = null;
	let requestKey = '';
	let requestToken = 0;
	let requestController: AbortController | null = null;
	let renderedPalette: ChartPalette | null = null;
	let renderedTimelines: CoverageStripTimeline[] = [];

	const startEpoch = $derived(dateStringToEpochPST(props.startDate));
	const endEpoch = $derived(dateStringToEpochPST(props.endDate, true));
	const selectedRouters = $derived(deriveSelectedRouters(props.routers));
	const visibleTimelines = $derived(trimCoverageTimelines(timelines));
	const chartHeight = $derived(Math.max(48, visibleTimelines.length * 18 + 30));

	function deriveSelectedRouters(routers: RouterConfig): string[] {
		return Object.entries(routers)
			.filter(([, enabled]) => enabled)
			.map(([router]) => router.trim())
			.filter((router) => router.length > 0)
			.sort();
	}

	function isRecord(value: unknown): value is Record<string, unknown> {
		return typeof value === 'object' && value !== null;
	}

	function isCoverageState(value: unknown): value is CoverageState {
		return value === 'complete' || value === 'partial' || value === 'unknown';
	}

	function toNumber(value: unknown, fallback: number): number {
		return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
	}

	function parseCoverage(value: unknown): BucketCoverage {
		if (!isRecord(value)) {
			return { state: 'unknown', observedUnits: 0, expectedUnits: 0 };
		}

		const state = isCoverageState(value.state) ? value.state : 'unknown';
		return {
			state,
			observedUnits: toNumber(value.observedUnits, 0),
			expectedUnits: toNumber(value.expectedUnits, 0)
		};
	}

	function parseCoverageTimelines(value: unknown): CoverageStripTimeline[] {
		if (!isRecord(value) || !Array.isArray(value.timelines)) return [];

		return value.timelines.flatMap((rawTimeline): CoverageStripTimeline[] => {
			if (!isRecord(rawTimeline)) return [];
			const sourceId = typeof rawTimeline.sourceId === 'string' ? rawTimeline.sourceId : '';
			if (!sourceId || !Array.isArray(rawTimeline.buckets)) return [];

			const buckets = rawTimeline.buckets.flatMap((rawBucket): CoverageStripBucket[] => {
				if (!isRecord(rawBucket)) return [];
				const bucketStart = toNumber(rawBucket.bucketStart, Number.NaN);
				const bucketEnd = toNumber(rawBucket.bucketEnd, Number.NaN);
				if (
					!Number.isFinite(bucketStart) ||
					!Number.isFinite(bucketEnd) ||
					bucketEnd <= bucketStart
				) {
					return [];
				}
				return [{ bucketStart, bucketEnd, coverage: parseCoverage(rawBucket.coverage) }];
			});

			return [{ sourceId, buckets }];
		});
	}

	function trimCoverageTimelines(
		coverageTimelines: CoverageStripTimeline[]
	): CoverageStripTimeline[] {
		const bounds = findTemporalDataBounds(
			coverageTimelines.flatMap((timeline) => timeline.buckets),
			(bucket) => bucket.bucketStart,
			(bucket) => bucket.coverage.state !== 'unknown'
		);
		if (!bounds) return [];

		return coverageTimelines.map((timeline) => ({
			...timeline,
			buckets: timeline.buckets.filter(
				(bucket) => bucket.bucketStart >= bounds.min && bucket.bucketStart <= bounds.max
			)
		}));
	}

	function getCacheKey(routers: string[]): string {
		return JSON.stringify({
			chart: CHART_ID,
			dataset: props.dataset,
			groupBy: props.groupBy,
			routers
		});
	}

	function readCachedCoverage(
		cacheKey: string,
		requestedRange: TimeRange,
		routers: string[]
	): CoverageStripTimeline[] {
		const records = readCachedWindow<CachedCoverageRecord>(
			cacheKey,
			requestedRange,
			(record, range) =>
				record.bucket.bucketStart >= range.start && record.bucket.bucketStart < range.end
		);
		return rebuildCoverageTimelines(records, routers);
	}

	async function loadCoverage(routers: string[], token: number) {
		requestController?.abort();
		const controller = new AbortController();
		requestController = controller;
		const requestedRange = { start: startEpoch, end: endEpoch };
		const cacheKey = getCacheKey(routers);
		loading = getMissingWindowRanges(cacheKey, requestedRange).length > 0;
		error = null;
		const baseParams = new URLSearchParams({
			dataset: props.dataset,
			groupBy: props.groupBy,
			routers: routers.join(',')
		});

		try {
			await ensureCachedWindow<CachedCoverageRecord>({
				key: cacheKey,
				requestedRange,
				signal: controller.signal,
				fetchRange: async (range, signal) => {
					const response = await fetch(
						`/api/netflow/coverage?${new URLSearchParams({
							...Object.fromEntries(baseParams.entries()),
							startDate: range.start.toString(),
							endDate: range.end.toString()
						}).toString()}`,
						{ signal }
					);
					if (!response.ok) {
						const message = await response.text();
						throw new Error(message || `Failed to load coverage: ${response.statusText}`);
					}
					const payload: unknown = await response.json();
					return flattenCoverageTimelines(parseCoverageTimelines(payload));
				},
				getRecordKey: (record) => `${record.sourceId}:${record.bucket.bucketStart}`,
				compareRecords: (left, right) =>
					left.bucket.bucketStart - right.bucket.bucketStart ||
					left.sourceId.localeCompare(right.sourceId)
			});
			if (token !== requestToken) return;
			timelines = readCachedCoverage(cacheKey, requestedRange, routers);
		} catch (err) {
			if (token !== requestToken) return;
			if (err instanceof DOMException && err.name === 'AbortError') return;
			timelines = [];
			error = err instanceof Error ? err.message : 'Failed to load coverage';
		} finally {
			if (token === requestToken) {
				loading = false;
				if (requestController === controller) requestController = null;
			}
		}
	}

	function getChartColors(): ChartPalette {
		const style = getComputedStyle(document.documentElement);
		return {
			textColor: style.getPropertyValue('--chart-text-color').trim(),
			trackColor: style.getPropertyValue('--chart-grid-color').trim(),
			tooltipBackgroundColor: style.getPropertyValue('--chart-tooltip-bg').trim(),
			tooltipTextColor: style.getPropertyValue('--chart-tooltip-text-color').trim(),
			tooltipBorderColor: style.getPropertyValue('--chart-tooltip-border-color').trim(),
			completeColor: 'rgb(16, 185, 129)',
			partialColor: 'rgb(245, 158, 11)'
		};
	}

	const coverageLanesPlugin: Plugin<'line'> = {
		id: 'coverageLanes',
		beforeDatasetsDraw(chartInstance) {
			const palette = renderedPalette;
			if (!palette) return;
			const xScale = chartInstance.scales.x;
			const yScale = chartInstance.scales.y;
			const { ctx, chartArea } = chartInstance;
			const bucketCount = renderedTimelines[0]?.buckets.length ?? 0;
			if (!xScale || !yScale || bucketCount === 0) return;

			const centers = Array.from({ length: bucketCount }, (_, index) =>
				xScale.getPixelForValue(index)
			);

			ctx.save();
			ctx.lineCap = 'butt';
			for (const [sourceIndex, timeline] of renderedTimelines.entries()) {
				const y = Math.round(yScale.getPixelForValue(sourceIndex)) + 0.5;
				ctx.strokeStyle = palette.trackColor;
				ctx.lineWidth = 1;
				ctx.setLineDash([]);
				ctx.beginPath();
				ctx.moveTo(chartArea.left, y);
				ctx.lineTo(chartArea.right, y);
				ctx.stroke();

				for (const [bucketIndex, bucket] of timeline.buckets.entries()) {
					if (bucket.coverage.state === 'unknown') continue;
					const center = centers[bucketIndex];
					if (center === undefined) continue;
					const previousCenter = centers[bucketIndex - 1];
					const nextCenter = centers[bucketIndex + 1];
					const left =
						previousCenter === undefined ? chartArea.left : (previousCenter + center) / 2;
					const right = nextCenter === undefined ? chartArea.right : (center + nextCenter) / 2;

					ctx.strokeStyle =
						bucket.coverage.state === 'complete' ? palette.completeColor : palette.partialColor;
					ctx.lineWidth = 2;
					ctx.setLineDash(bucket.coverage.state === 'partial' ? [5, 4] : []);
					ctx.beginPath();
					ctx.moveTo(left, y);
					ctx.lineTo(right, y);
					ctx.stroke();
				}
			}
			ctx.restore();
		}
	};

	function destroyChart() {
		if (!chart) return;
		crosshairStore.unregister(CHART_ID);
		chart.destroy();
		chart = null;
	}

	function renderChart() {
		if (!canvas || visibleTimelines.length === 0 || visibleTimelines[0]?.buckets.length === 0) {
			destroyChart();
			return;
		}

		const palette = getChartColors();
		const labels = visibleTimelines[0].buckets.map((bucket) =>
			formatCoverageStripLabel(bucket.bucketStart, props.groupBy)
		);
		const datasets = visibleTimelines.map((timeline, sourceIndex) => ({
			label: timeline.sourceId,
			data: timeline.buckets.map(() => sourceIndex),
			borderColor: 'transparent',
			backgroundColor: 'transparent',
			borderWidth: 0,
			pointRadius: 0,
			pointHoverRadius: 0,
			pointHitRadius: 10,
			showLine: false
		}));

		renderedPalette = palette;
		renderedTimelines = visibleTimelines;
		const config = {
			type: 'line',
			data: { labels, datasets },
			plugins: [coverageLanesPlugin],
			options: {
				responsive: true,
				maintainAspectRatio: false,
				animation: false,
				normalized: true,
				interaction: { mode: 'index', axis: 'x', intersect: false },
				layout: { padding: { top: 4, right: 4, bottom: 4 } },
				plugins: {
					legend: { display: false },
					tooltip: {
						mode: 'index',
						intersect: false,
						displayColors: false,
						backgroundColor: palette.tooltipBackgroundColor,
						titleColor: palette.tooltipTextColor,
						bodyColor: palette.tooltipTextColor,
						borderColor: palette.tooltipBorderColor,
						borderWidth: 1,
						callbacks: {
							label: (context: TooltipItem<'line'>) => {
								const timeline = visibleTimelines[context.datasetIndex];
								const bucket = timeline?.buckets[context.dataIndex];
								return bucket
									? `${timeline.sourceId}: ${formatCoverageState(bucket.coverage)}`
									: '';
							}
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
							enabled: false,
							delay: 500,
							backgroundColor: palette.tooltipBackgroundColor,
							textColor: palette.tooltipTextColor,
							borderColor: palette.tooltipBorderColor,
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
				} as Record<string, unknown>,
				scales: {
					x: {
						type: 'category',
						offset: true,
						ticks: { display: false },
						grid: { display: false },
						border: { display: false }
					},
					y: {
						type: 'linear',
						min: -0.5,
						max: visibleTimelines.length - 0.5,
						reverse: true,
						afterBuildTicks: (axis: { ticks: Array<{ value: number }> }) => {
							axis.ticks = visibleTimelines.map((_, index) => ({ value: index }));
						},
						ticks: {
							color: palette.textColor,
							font: { size: 10 },
							padding: 6,
							callback: (value: string | number) => visibleTimelines[Number(value)]?.sourceId ?? ''
						},
						grid: { display: false },
						border: { display: false }
					}
				}
			}
		};
		if (chart && chart.canvas === canvas) {
			chart.data = config.data;
			chart.options = config.options as never;
			chart.update('none');
			return;
		}

		destroyChart();
		chart = new Chart(canvas, config as never);
		crosshairStore.register(CHART_ID, chart);
	}

	$effect(() => {
		if (!props.routersLoaded) {
			requestToken += 1;
			requestController?.abort();
			requestController = null;
			requestKey = '';
			loading = true;
			error = null;
			timelines = [];
			return;
		}

		if (selectedRouters.length === 0) {
			requestToken += 1;
			requestController?.abort();
			requestController = null;
			requestKey = '';
			loading = false;
			error = 'Select at least one source to view coverage';
			timelines = [];
			return;
		}

		const nextKey = JSON.stringify({
			dataset: props.dataset,
			startDate: props.startDate,
			endDate: props.endDate,
			groupBy: props.groupBy,
			routers: selectedRouters
		});
		if (nextKey === requestKey) return;
		requestKey = nextKey;
		const token = ++requestToken;
		void loadCoverage(selectedRouters, token);
	});

	$effect(() => {
		void theme.dark;
		void timelines;
		renderChart();
	});

	onDestroy(() => {
		requestToken += 1;
		requestController?.abort();
		if (crosshairStore.sourceChartId === CHART_ID) {
			crosshairStore.clearHover();
		}
		destroyChart();
	});
</script>

<div class="bg-card rounded-lg border shadow-sm" data-testid="coverage-strip-card">
	<div
		class="relative cursor-grab border-b p-3 select-none active:cursor-grabbing"
		draggable="true"
		data-drag-handle
	>
		<h2 class="text-foreground text-sm font-semibold">Coverage</h2>
		<DragGrip />
	</div>

	<div class="px-3 py-2">
		{#if loading}
			<div class="text-muted-foreground py-1 text-xs">Loading coverage...</div>
		{:else if error}
			<div class="py-1 text-xs text-red-500">{error}</div>
		{:else if visibleTimelines.length === 0}
			<div class="text-muted-foreground py-1 text-xs">No coverage available</div>
		{:else}
			<div
				class="relative"
				style={`height:${chartHeight}px`}
				data-testid="coverage-strip"
				role="region"
				aria-label="Coverage timeline"
			>
				<canvas bind:this={canvas} aria-label="Coverage time series"></canvas>
			</div>
			<p class="sr-only">
				{visibleTimelines.length} source coverage lanes across {visibleTimelines[0]?.buckets
					.length ?? 0} time buckets.
			</p>
		{/if}
	</div>
</div>
