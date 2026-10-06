<script module lang="ts">
	import type { CoverageTimeline, CoverageTimelineBucket } from '#lib/types/types.ts';
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
	import DragGrip from '#lib/components/common/DragGrip.svelte';
	import type { BucketCoverage, CoverageState } from '#lib/types/types.ts';
	import type { GroupByOption, RouterConfig } from '#lib/components/netflow/types.ts';
	import { dateStringToEpochPST } from '#lib/utils/timezone.ts';
	import { ensureCachedWindow, readCachedWindow, type TimeRange } from '#lib/utils/window-cache.ts';
	import { findTemporalDataBounds } from './chart-utils';
	import {
		formatCoverageState,
		formatCoverageStripLabel,
		type CoverageStripBucket,
		type CoverageStripTimeline
	} from './coverage-strip';
	import ChartPlot from './ChartPlot.svelte';
	import type { PlotSeries } from './chart-registry';
	const CHART_ID = 'coverage-strip';
	const props = $props<{
		dataset: string;
		startDate: string;
		endDate: string;
		groupBy: GroupByOption;
		routers: RouterConfig;
		routersLoaded: boolean;
	}>();
	const startEpoch = $derived(dateStringToEpochPST(props.startDate));
	const endEpoch = $derived(dateStringToEpochPST(props.endDate, true));
	const selectedRouters = $derived(deriveSelectedRouters(props.routers));
	const requestKey = $derived(
		JSON.stringify({
			dataset: props.dataset,
			startEpoch,
			endEpoch,
			groupBy: props.groupBy,
			routers: selectedRouters
		})
	);
	let settled = $state.raw<{
		key: string;
		timelines: CoverageStripTimeline[];
		error: string | null;
	} | null>(null);
	const loading = $derived(
		!props.routersLoaded || (selectedRouters.length > 0 && settled?.key !== requestKey)
	);
	const error = $derived(
		selectedRouters.length === 0 && props.routersLoaded
			? 'Select at least one source to view coverage'
			: settled?.key === requestKey
				? settled.error
				: null
	);
	const timelines = $derived(settled?.key === requestKey ? settled.timelines : []);
	const visibleTimelines = $derived(trimCoverageTimelines(timelines));
	const chartHeight = $derived(Math.max(48, visibleTimelines.length * 18 + 30));
	const series = $derived<PlotSeries[]>(
		visibleTimelines.map((timeline, index) => ({
			label: timeline.sourceId,
			color: 'rgb(16,185,129)',
			data: timeline.buckets.map((bucket) => ({
				x: bucket.bucketStart,
				x2: bucket.bucketEnd,
				y: index,
				label: formatCoverageStripLabel(bucket.bucketStart, props.groupBy),
				coverage: bucket.coverage,
				value:
					bucket.coverage.state === 'unknown' || bucket.coverage.expectedUnits === 0
						? null
						: bucket.coverage.observedUnits / bucket.coverage.expectedUnits
			}))
		}))
	);
	const domain = $derived.by((): readonly [number, number] => {
		let min = Infinity;
		let max = -Infinity;
		for (const item of series)
			for (const point of item.data) {
				min = Math.min(min, point.x);
				max = Math.max(max, point.x2 ?? point.x);
			}
		return Number.isFinite(min) ? [min, max] : [0, 1];
	});
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

	async function loadCoverage(routers: string[], signal: AbortSignal) {
		const range = { start: startEpoch, end: endEpoch };
		const cacheKey = getCacheKey(routers);
		const params = new URLSearchParams({
			dataset: props.dataset,
			groupBy: props.groupBy,
			routers: routers.join(',')
		});
		await ensureCachedWindow<CachedCoverageRecord>({
			key: cacheKey,
			requestedRange: range,
			signal,
			fetchRange: async (range, signal) => {
				const response = await fetch(
					`/api/netflow/coverage?${new URLSearchParams({ ...Object.fromEntries(params), startDate: String(range.start), endDate: String(range.end) })}`,
					{ signal }
				);
				if (!response.ok) throw new Error((await response.text()) || 'Failed to load coverage');
				return flattenCoverageTimelines(parseCoverageTimelines(await response.json()));
			},
			getRecordKey: (record) => `${record.sourceId}:${record.bucket.bucketStart}`,
			compareRecords: (left, right) =>
				left.bucket.bucketStart - right.bucket.bucketStart ||
				left.sourceId.localeCompare(right.sourceId)
		});
		return readCachedCoverage(cacheKey, range, routers);
	}
	$effect(() => {
		const key = requestKey;
		if (!props.routersLoaded || selectedRouters.length === 0) return;
		const controller = new AbortController();
		loadCoverage(selectedRouters, controller.signal).then(
			(timelines) => {
				if (!controller.signal.aborted) settled = { key, timelines, error: null };
			},
			(reason: unknown) => {
				if (!controller.signal.aborted)
					settled = {
						key,
						timelines: [],
						error: reason instanceof Error ? reason.message : 'Failed to load coverage'
					};
			}
		);
		return () => controller.abort();
	});
</script>

<div
	class="bg-card rounded-lg border shadow-sm"
	data-testid="coverage-strip-card"
	data-state={loading
		? 'loading'
		: error
			? 'error'
			: visibleTimelines.length === 0
				? 'empty'
				: 'ready'}
>
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
				<ChartPlot
					name="Coverage time series"
					chartId={CHART_ID}
					{series}
					options={{
						kind: 'coverage',
						xTitle: '',
						yTitle: '',
						xDomain: domain,
						yDomain: [visibleTimelines.length - 0.5, -0.5],
						yTicks: visibleTimelines.map((_, index) => index),
						yFormat: (value) => visibleTimelines[value]?.sourceId ?? '',
						legend: false
					}}
					formatTooltip={(points) =>
						[
							points[0]?.datum.label ?? '',
							...points.map(
								(point) =>
									`${point.groupLabel}: ${point.datum.coverage ? formatCoverageState(point.datum.coverage) : ''}`
							)
						].join('\n')}
				/>
			</div>
			<p class="sr-only">
				{visibleTimelines.length} source coverage lanes across {visibleTimelines[0]?.buckets
					.length ?? 0} time buckets.
			</p>
		{/if}
	</div>
</div>
