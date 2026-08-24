import { onDestroy } from 'svelte';
import { SvelteMap, SvelteURLSearchParams } from 'svelte/reactivity';
import { watch } from 'runed';
import { dateStringToEpochPST } from '$lib/utils/timezone';
import type { GroupByOption, RouterConfig } from '$lib/components/netflow/types';
import type {
	FlowCharacteristicsResponse,
	FlowVisibility,
	IpGranularity,
	ObservationStats,
	PortCardinalityCounts,
	TimeBucket
} from '$lib/types/types';
import {
	ensureCachedWindow,
	getMissingWindowRanges,
	readCachedWindow,
	type TimeRange
} from '$lib/utils/window-cache';
import { createRequestGate } from './flow-characteristics';

export type FlowCharacteristicsFilters = {
	enabled: boolean;
	dataset: string;
	startDate: string;
	endDate: string;
	groupBy: GroupByOption;
	routers: RouterConfig;
	routersLoaded: boolean;
	srcVisibility: FlowVisibility;
	dstVisibility: FlowVisibility;
};

export type FlowCharacteristicsData = {
	readonly data: FlowCharacteristicsResponse | null;
	readonly loading: boolean;
	readonly error: string | null;
};

type CachedCharacteristicsRecord =
	| { kind: 'source'; sourceId: string; sourceIndex: number }
	| { kind: 'observation'; bucket: TimeBucket<ObservationStats[]> }
	| { kind: 'port'; sourceId: string; bucket: TimeBucket<PortCardinalityCounts> };

const GROUP_BY_TO_GRANULARITY: Record<GroupByOption, IpGranularity> = {
	date: '1d',
	hour: '1h',
	'30min': '30m',
	'5min': '5m'
};

function selectedSources(routers: RouterConfig): string[] {
	return Object.entries(routers)
		.filter(([, enabled]) => enabled)
		.map(([sourceId]) => sourceId.trim())
		.filter(Boolean)
		.sort();
}

function cacheKey(filters: FlowCharacteristicsFilters, routers: string[]): string {
	return JSON.stringify({
		chart: 'flow-characteristics',
		dataset: filters.dataset,
		granularity: GROUP_BY_TO_GRANULARITY[filters.groupBy],
		routers,
		srcVisibility: filters.srcVisibility,
		dstVisibility: filters.dstVisibility
	});
}

function recordStart(record: CachedCharacteristicsRecord): number {
	return record.kind === 'source' ? Number.NEGATIVE_INFINITY : record.bucket.bucketStart;
}

function readCachedData(key: string, requestedRange: TimeRange): FlowCharacteristicsResponse {
	const records = readCachedWindow<CachedCharacteristicsRecord>(
		key,
		requestedRange,
		(record, range) =>
			record.kind === 'source' ||
			(record.bucket.bucketStart >= range.start && record.bucket.bucketStart < range.end)
	);
	const sources = records
		.filter(
			(record): record is Extract<CachedCharacteristicsRecord, { kind: 'source' }> =>
				record.kind === 'source'
		)
		.sort(
			(left, right) =>
				left.sourceIndex - right.sourceIndex || left.sourceId.localeCompare(right.sourceId)
		);
	const portBuckets = new SvelteMap<string, TimeBucket<PortCardinalityCounts>[]>();
	for (const source of sources) portBuckets.set(source.sourceId, []);
	for (const record of records) {
		if (record.kind !== 'port') continue;
		const buckets = portBuckets.get(record.sourceId) ?? [];
		buckets.push(record.bucket);
		portBuckets.set(record.sourceId, buckets);
	}

	return {
		observationBuckets: records
			.filter(
				(record): record is Extract<CachedCharacteristicsRecord, { kind: 'observation' }> =>
					record.kind === 'observation'
			)
			.map((record) => record.bucket)
			.sort((left, right) => left.bucketStart - right.bucketStart),
		portTimelines: [...portBuckets].map(([sourceId, buckets]) => ({
			sourceId,
			buckets: buckets.sort((left, right) => left.bucketStart - right.bucketStart)
		})),
		resolvedSources: sources.map((source) => source.sourceId)
	};
}

/** Share one cached characteristics request between the observation and port cards. */
export function createFlowCharacteristicsData(
	getFilters: () => FlowCharacteristicsFilters
): FlowCharacteristicsData {
	let data = $state.raw<FlowCharacteristicsResponse | null>(null);
	let loading = $state(false);
	let error = $state<string | null>(null);
	const requestGate = createRequestGate();
	let requestController: AbortController | null = null;

	async function loadData(filters: FlowCharacteristicsFilters) {
		const token = requestGate.begin();
		requestController?.abort();
		requestController = null;

		if (!filters.enabled) {
			data = null;
			error = null;
			loading = false;
			return;
		}
		if (!filters.routersLoaded) {
			data = null;
			error = null;
			loading = true;
			return;
		}

		const routers = selectedSources(filters.routers);
		if (routers.length === 0) {
			data = null;
			error = 'Select at least one source to view flow characteristics';
			loading = false;
			return;
		}

		const granularity = GROUP_BY_TO_GRANULARITY[filters.groupBy];
		const requestedRange = {
			start: dateStringToEpochPST(filters.startDate),
			end: dateStringToEpochPST(filters.endDate, true)
		};
		const key = cacheKey(filters, routers);
		loading = getMissingWindowRanges(key, requestedRange).length > 0;
		error = null;
		const baseParams = new SvelteURLSearchParams({
			dataset: filters.dataset,
			routers: routers.join(','),
			granularity,
			srcVisibility: filters.srcVisibility,
			dstVisibility: filters.dstVisibility
		});
		const controller = new AbortController();
		requestController = controller;

		try {
			await ensureCachedWindow<CachedCharacteristicsRecord>({
				key,
				requestedRange,
				signal: controller.signal,
				fetchRange: async (range, signal) => {
					const params = new SvelteURLSearchParams({
						...Object.fromEntries(baseParams.entries()),
						startDate: range.start.toString(),
						endDate: range.end.toString()
					});
					const response = await fetch(`/api/netflow/characteristics?${params}`, { signal });
					if (!response.ok) throw new Error((await response.text()) || 'Request failed');
					const next = (await response.json()) as FlowCharacteristicsResponse;
					return [
						...next.resolvedSources.map(
							(sourceId, sourceIndex): CachedCharacteristicsRecord => ({
								kind: 'source',
								sourceId,
								sourceIndex
							})
						),
						...next.observationBuckets.map(
							(bucket): CachedCharacteristicsRecord => ({ kind: 'observation', bucket })
						),
						...next.portTimelines.flatMap((timeline) =>
							timeline.buckets.map(
								(bucket): CachedCharacteristicsRecord => ({
									kind: 'port',
									sourceId: timeline.sourceId,
									bucket
								})
							)
						)
					];
				},
				getRecordKey: (record) => {
					if (record.kind === 'source') return `source:${record.sourceId}`;
					if (record.kind === 'observation') return `observation:${record.bucket.bucketStart}`;
					return `port:${record.sourceId}:${record.bucket.bucketStart}`;
				},
				compareRecords: (left, right) =>
					recordStart(left) - recordStart(right) ||
					(left.kind === 'port' ? left.sourceId : '').localeCompare(
						right.kind === 'port' ? right.sourceId : ''
					)
			});
			if (requestGate.isCurrent(token)) data = readCachedData(key, requestedRange);
		} catch (reason) {
			if (requestGate.isCurrent(token)) {
				if (reason instanceof DOMException && reason.name === 'AbortError') return;
				data = null;
				error = reason instanceof Error ? reason.message : 'Failed to load flow characteristics';
			}
		} finally {
			if (requestGate.isCurrent(token)) {
				loading = false;
				if (requestController === controller) requestController = null;
			}
		}
	}

	watch(
		() => JSON.stringify(getFilters()),
		() => void loadData(getFilters())
	);

	onDestroy(() => {
		requestGate.begin();
		requestController?.abort();
	});

	return {
		get data() {
			return data;
		},
		get loading() {
			return loading;
		},
		get error() {
			return error;
		}
	};
}
