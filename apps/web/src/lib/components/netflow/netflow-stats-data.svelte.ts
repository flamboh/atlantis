import { dateStringToEpochPST } from '#lib/utils/timezone.ts';
import type { GroupByOption, RouterConfig } from '#lib/components/netflow/types.ts';
import type {
	FlowDirection,
	NetflowIpFamily,
	NetflowStatsResponse,
	NetflowStatsResult,
	TimeBucket
} from '#lib/types/types.ts';
import { ensureCachedWindow, readCachedWindow, type TimeRange } from '#lib/utils/window-cache.ts';
import { createRequestGate } from '#lib/components/charts/flow-characteristics.ts';

export type NetflowStatsFilters = {
	dataset: string;
	startDate: string;
	endDate: string;
	groupBy: GroupByOption;
	routers: RouterConfig;
	routersLoaded: boolean;
	direction: FlowDirection;
};

export type NetflowStatsData = {
	readonly results: TimeBucket<NetflowStatsResult>[];
	readonly availableIpFamilies: NetflowIpFamily[];
	readonly loading: boolean;
	readonly error: string | null;
};

type StatsRequest =
	| { kind: 'waiting' }
	| { kind: 'invalid'; error: string }
	| {
			kind: 'fetch';
			id: string;
			key: string;
			requestedRange: TimeRange;
			baseParams: Record<string, string>;
	  };

type FetchRequest = Extract<StatsRequest, { kind: 'fetch' }>;

type SettledRequest = {
	id: string;
	results: TimeBucket<NetflowStatsResult>[];
	availableIpFamilies: NetflowIpFamily[];
	error: string | null;
};

const ipFamilyCache: Record<string, NetflowIpFamily[]> = {};
const ALL_FAMILIES: NetflowIpFamily[] = ['all'];

function selectedRouters(routers: RouterConfig): string[] {
	return Object.entries(routers)
		.filter(([, enabled]) => enabled)
		.map(([router]) => router.trim())
		.filter((router) => router.length > 0)
		.sort();
}

function describeRequest(filtersKey: string): StatsRequest {
	const filters = JSON.parse(filtersKey) as NetflowStatsFilters;
	if (!filters.routersLoaded) return { kind: 'waiting' };
	const routers = selectedRouters(filters.routers);
	if (routers.length === 0) {
		return { kind: 'invalid', error: 'Select at least one source to view NetFlow statistics' };
	}
	const key = JSON.stringify({
		chart: 'netflow',
		dataset: filters.dataset,
		groupBy: filters.groupBy,
		routers,
		direction: filters.direction
	});
	const requestedRange = {
		start: dateStringToEpochPST(filters.startDate),
		end: dateStringToEpochPST(filters.endDate, true)
	};
	if (!Number.isFinite(requestedRange.start) || !Number.isFinite(requestedRange.end)) {
		return { kind: 'invalid', error: 'Invalid start or end date' };
	}
	if (requestedRange.start >= requestedRange.end) {
		return { kind: 'invalid', error: 'Start Date must be on or before End Date.' };
	}
	return {
		kind: 'fetch',
		id: filtersKey,
		key,
		requestedRange,
		baseParams: {
			dataset: filters.dataset,
			routers: routers.join(','),
			groupBy: filters.groupBy,
			direction: filters.direction
		}
	};
}

async function fetchStats(
	request: FetchRequest,
	signal: AbortSignal
): Promise<Omit<SettledRequest, 'id' | 'error'>> {
	await ensureCachedWindow<TimeBucket<NetflowStatsResult>>({
		key: request.key,
		requestedRange: request.requestedRange,
		signal,
		fetchRange: async (range, signal) => {
			const params = new URLSearchParams({
				...request.baseParams,
				startDate: range.start.toString(),
				endDate: range.end.toString()
			});
			const response = await fetch(`/api/netflow/stats?${params}`, { signal });
			if (!response.ok) {
				const message = await response.text();
				throw new Error(message || `Failed to load data: ${response.statusText}`);
			}
			const json = (await response.json()) as NetflowStatsResponse;
			ipFamilyCache[request.key] = json.availableIpFamilies;
			return json.result;
		},
		getRecordKey: (record) => `${record.bucketStart}`,
		compareRecords: (left, right) => left.bucketStart - right.bucketStart
	});
	return {
		results: readCachedWindow<TimeBucket<NetflowStatsResult>>(
			request.key,
			request.requestedRange,
			(record, range) => record.bucketStart >= range.start && record.bucketStart < range.end
		),
		availableIpFamilies: ipFamilyCache[request.key] ?? ALL_FAMILIES
	};
}

/** Share one cached /api/netflow/stats window between the KPI row and the traffic card. */
export function createNetflowStatsData(getFilters: () => NetflowStatsFilters): NetflowStatsData {
	const filtersKey = $derived(JSON.stringify(getFilters()));
	const request = $derived(describeRequest(filtersKey));
	let settled = $state.raw<SettledRequest | null>(null);
	const requestGate = createRequestGate();

	$effect(() => {
		if (request.kind !== 'fetch') return;
		const current = request;
		const token = requestGate.begin();
		const controller = new AbortController();
		fetchStats(current, controller.signal).then(
			(data) => {
				if (requestGate.isCurrent(token)) settled = { id: current.id, ...data, error: null };
			},
			(reason: unknown) => {
				if (!requestGate.isCurrent(token)) return;
				if (reason instanceof DOMException && reason.name === 'AbortError') return;
				settled = {
					id: current.id,
					results: [],
					availableIpFamilies: ALL_FAMILIES,
					error: `Failed to load data: ${reason instanceof Error ? reason.message : 'Unknown error'}`
				};
			}
		);
		return () => {
			requestGate.begin();
			controller.abort();
		};
	});

	return {
		get results() {
			return request.kind === 'fetch' && settled?.id === request.id ? settled.results : [];
		},
		get availableIpFamilies() {
			return request.kind === 'fetch'
				? settled?.id === request.id
					? settled.availableIpFamilies
					: (ipFamilyCache[request.key] ?? ALL_FAMILIES)
				: ALL_FAMILIES;
		},
		get loading() {
			if (request.kind === 'waiting') return true;
			return request.kind === 'fetch' && settled?.id !== request.id;
		},
		get error() {
			if (request.kind === 'invalid') return request.error;
			return request.kind === 'fetch' && settled?.id === request.id ? settled.error : null;
		}
	};
}
