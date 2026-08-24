import type {
	BucketCoverage,
	NetflowIpFamily,
	ObservationStats,
	PortCardinalityCounts,
	PortCardinalityTimeline,
	TimeBucket
} from '$lib/types/types';

export interface RequestGate {
	begin(): number;
	isCurrent(token: number): boolean;
}

export function createRequestGate(): RequestGate {
	let currentToken = 0;
	return {
		begin: () => ++currentToken,
		isCurrent: (token) => token === currentToken
	};
}

const SOURCE_LINE_DASHES: number[][] = [[], [8, 3], [3, 3], [10, 3, 2, 3]];

export function getSourceLineDash(sourceIndex: number, multipleSources: boolean): number[] {
	if (!multipleSources) return [];
	return SOURCE_LINE_DASHES[sourceIndex % SOURCE_LINE_DASHES.length] ?? [];
}

export type IndexedObservationBucket = {
	coverage: BucketCoverage;
	byFamily: Map<NetflowIpFamily, ObservationStats>;
};

export type IndexedPortBucket = {
	coverage: BucketCoverage;
	values: PortCardinalityCounts | null;
};

/** Index observation rows once so each rendered series can read by bucket and family. */
export function indexObservationBuckets(buckets: TimeBucket<ObservationStats[]>[]) {
	const byStart = new Map<number, IndexedObservationBucket>();
	for (const bucket of buckets) {
		byStart.set(bucket.bucketStart, {
			coverage: bucket.coverage,
			byFamily: new Map((bucket.data ?? []).map((row) => [row.ipFamily, row]))
		});
	}
	return { starts: [...byStart.keys()].sort((left, right) => left - right), byStart };
}

/** Index exact port counts by source and bucket without adding logical sources together. */
export function indexPortTimelines(timelines: PortCardinalityTimeline[]) {
	const starts = new Set<number>();
	const bySource = new Map<string, Map<number, IndexedPortBucket>>();
	for (const timeline of timelines) {
		const bucketsByStart = bySource.get(timeline.sourceId) ?? new Map();
		bySource.set(timeline.sourceId, bucketsByStart);
		for (const bucket of timeline.buckets) {
			starts.add(bucket.bucketStart);
			bucketsByStart.set(bucket.bucketStart, {
				coverage: bucket.coverage,
				values: bucket.data
			});
		}
	}
	return { starts: [...starts].sort((left, right) => left - right), bySource };
}
