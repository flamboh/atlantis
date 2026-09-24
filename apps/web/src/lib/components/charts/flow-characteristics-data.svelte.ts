import { SvelteMap, SvelteURLSearchParams } from 'svelte/reactivity';
import { dateStringToEpochPST } from '#lib/utils/timezone.ts';
import type { GroupByOption, RouterConfig } from '#lib/components/netflow/types.ts';
import type {
	FlowCharacteristicsResponse,
	FlowVisibility,
	IpGranularity,
	ObservationStats,
	PortCardinalityCounts,
	TimeBucket
} from '#lib/types/types.ts';
import {
	ensureCachedWindow,
	getMissingWindowRanges,
	readCachedWindow,
	type TimeRange
} from '#lib/utils/window-cache.ts';
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

type CharacteristicsRequest =
	| { kind: 'idle' }
	| { kind: 'waiting' }
	| { kind: 'invalid'; error: string }
	| {
			kind: 'fetch';
			id: string;
			key: string;
			requestedRange: TimeRange;
			baseParams: Record<string, string>;
			missing: boolean;
	  };

type FetchRequest = Extract<CharacteristicsRequest, { kind: 'fetch' }>;

type SettledRequest = {
	id: string;
	data: FlowCharacteristicsResponse | null;
	error: string | null;
};

function describeRequest(filtersKey: string): CharacteristicsRequest {
	const filters = JSON.parse(filtersKey) as FlowCharacteristicsFilters;
	if (!filters.enabled) return { kind: 'idle' };
	if (!filters.routersLoaded) return { kind: 'waiting' };

	const routers = selectedSources(filters.routers);
	if (routers.length === 0) {
		return { kind: 'invalid', error: 'Select at least one source to view flow characteristics' };
	}

	const requestedRange = {
		start: dateStringToEpochPST(filters.startDate),
		end: dateStringToEpochPST(filters.endDate, true)
	};
	const key = cacheKey(filters, routers);
	return {
		kind: 'fetch',
		id: filtersKey,
		key,
		requestedRange,
		baseParams: {
			dataset: filters.dataset,
			routers: routers.join(','),
			granularity: GROUP_BY_TO_GRANULARITY[filters.groupBy],
			srcVisibility: filters.srcVisibility,
			dstVisibility: filters.dstVisibility
		},
		missing: getMissingWindowRanges(key, requestedRange).length > 0
	};
}

async function fetchCharacteristics(
	request: FetchRequest,
	signal: AbortSignal
): Promise<FlowCharacteristicsResponse> {
	await ensureCachedWindow<CachedCharacteristicsRecord>({
		key: request.key,
		requestedRange: request.requestedRange,
		signal,
		fetchRange: async (range, signal) => {
			const params = new SvelteURLSearchParams({
				...request.baseParams,
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
	return readCachedData(request.key, request.requestedRange);
}

/** Share one cached characteristics request between the observation and port cards. */
export function createFlowCharacteristicsData(
	getFilters: () => FlowCharacteristicsFilters
): FlowCharacteristicsData {
	const filtersKey = $derived(JSON.stringify(getFilters()));
	const request = $derived(describeRequest(filtersKey));
	let settled = $state.raw<SettledRequest | null>(null);
	const requestGate = createRequestGate();

	$effect(() => {
		if (request.kind !== 'fetch') return;
		const current = request;
		const token = requestGate.begin();
		const controller = new AbortController();
		fetchCharacteristics(current, controller.signal).then(
			(data) => {
				if (requestGate.isCurrent(token)) settled = { id: current.id, data, error: null };
			},
			(reason: unknown) => {
				if (!requestGate.isCurrent(token)) return;
				if (reason instanceof DOMException && reason.name === 'AbortError') return;
				settled = {
					id: current.id,
					data: null,
					error: reason instanceof Error ? reason.message : 'Failed to load flow characteristics'
				};
			}
		);
		return () => {
			requestGate.begin();
			controller.abort();
		};
	});

	return {
		get data() {
			return request.kind === 'fetch' ? (settled?.data ?? null) : null;
		},
		get loading() {
			if (request.kind === 'waiting') return true;
			return request.kind === 'fetch' && settled?.id !== request.id && request.missing;
		},
		get error() {
			if (request.kind === 'invalid') return request.error;
			return request.kind === 'fetch' && settled?.id === request.id ? settled.error : null;
		}
	};
}
