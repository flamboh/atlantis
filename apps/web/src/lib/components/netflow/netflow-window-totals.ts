import type { NetflowStatsResult, TimeBucket } from '#lib/types/types.ts';

export type NetflowWindowTotals = {
	flows: number;
	flowsTcp: number;
	packets: number;
	bytes: number;
	buckets: number;
	completeBuckets: number;
	partialBuckets: number;
	unknownBuckets: number;
};

/** Sum the additive totals of every observed bucket; buckets without data contribute nothing. */
export function sumNetflowWindow(results: readonly TimeBucket<NetflowStatsResult>[]) {
	const totals: NetflowWindowTotals = {
		flows: 0,
		flowsTcp: 0,
		packets: 0,
		bytes: 0,
		buckets: results.length,
		completeBuckets: 0,
		partialBuckets: 0,
		unknownBuckets: 0
	};
	for (const bucket of results) {
		if (bucket.coverage.state === 'complete') totals.completeBuckets += 1;
		else if (bucket.coverage.state === 'partial') totals.partialBuckets += 1;
		else totals.unknownBuckets += 1;
		if (!bucket.data) continue;
		totals.flows += bucket.data.flows;
		totals.flowsTcp += bucket.data.flowsTcp;
		totals.packets += bucket.data.packets;
		totals.bytes += bucket.data.bytes;
	}
	return totals;
}
