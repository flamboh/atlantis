import type { BucketCoverage, CoverageTimeline, CoverageTimelineBucket } from '$lib/types/types';
import type { GroupByOption } from '$lib/components/netflow/types';
import { epochToPSTComponents } from '$lib/utils/timezone';

export type CoverageStripBucket = CoverageTimelineBucket;
export type CoverageStripTimeline = CoverageTimeline;

/** Use the same Pacific-time labels as the main NetFlow chart. */
export function formatCoverageStripLabel(bucketStart: number, groupBy: GroupByOption): string {
	const pst = epochToPSTComponents(bucketStart);
	const date = `${pst.year}-${String(pst.month).padStart(2, '0')}-${String(pst.day).padStart(2, '0')}`;
	if (groupBy === 'date') {
		return date;
	}

	const hour = String(pst.hours).padStart(2, '0');
	if (groupBy === 'hour') {
		return `${date} ${hour}:00`;
	}

	return `${date} ${hour}:${String(pst.minutes).padStart(2, '0')}`;
}

export function formatCoverageState(coverage: BucketCoverage): string {
	if (coverage.state === 'complete') {
		return 'Complete coverage';
	}
	if (coverage.state === 'partial') {
		return `Partial coverage · ${coverage.observedUnits.toLocaleString()} / ${coverage.expectedUnits.toLocaleString()} units`;
	}
	return 'Unknown coverage';
}
