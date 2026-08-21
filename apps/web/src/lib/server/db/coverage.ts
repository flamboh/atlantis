import type { ReadonlyDatasetDb } from '$lib/server/datasets';
import type {
	BucketCoverage,
	CoverageState,
	CoverageTimeline,
	IpGranularity,
	TimeBucket
} from '$lib/types/types';
import { createDateFromPSTComponents, epochToPSTComponents } from '$lib/utils/timezone';

const GRANULARITY_SECONDS: Record<IpGranularity, number> = {
	'5m': 5 * 60,
	'30m': 30 * 60,
	'1h': 60 * 60,
	'1d': 24 * 60 * 60
};

export type CoverageTimelinePartition = {
	key: string;
	sourceIds: readonly string[];
};

type CoverageRow = {
	sourceId: string;
	bucketStart: number;
	bucketEnd: number;
	coverageState: string;
	observedUnits: number;
	expectedUnits: number;
	rejectedUnits: number;
};

type TimelineRow = {
	bucketStart: number;
};

type CoverageAggregate = {
	bucketEnd: number;
	state: CoverageState;
	observedUnits: number;
	expectedUnits: number;
	rejectedUnits: number;
	hasPartialRow: boolean;
	hasUnknownRow: boolean;
	hasCompleteRow: boolean;
};

export type BuildCoverageTimelinesOptions<TData, TRow extends TimelineRow> = {
	db: ReadonlyDatasetDb;
	granularity: IpGranularity;
	start: number;
	end: number;
	partitions: readonly CoverageTimelinePartition[];
	rows: readonly TRow[];
	getPartitionKey: (row: TRow) => string;
	toData: (row: TRow) => TData;
	emptyData: (partitionKey: string) => TData | null;
};

export type BuildCoverageOnlyTimelinesOptions = {
	db: ReadonlyDatasetDb;
	granularity: IpGranularity;
	start: number;
	end: number;
	sourceIds: readonly string[];
};

/**
 * Query explicit capture coverage and fill every requested bucket exactly once.
 * Missing coverage is unknown; only complete buckets without a metric row get
 * a numeric zero value from the route's `emptyData` factory.
 */
export async function buildCoverageTimelines<TData, TRow extends TimelineRow>(
	options: BuildCoverageTimelinesOptions<TData, TRow>
): Promise<Map<string, TimeBucket<TData>[]>> {
	const { db, granularity, start, end, partitions, rows, getPartitionKey, toData, emptyData } =
		options;
	const timelines = new Map<string, TimeBucket<TData>[]>();
	const rowsByPartition = new Map<string, Map<number, TRow>>();
	const coverageByPartition = new Map<string, Map<number, CoverageAggregate>>();

	for (const partition of partitions) {
		timelines.set(partition.key, []);
		rowsByPartition.set(partition.key, new Map());
		coverageByPartition.set(partition.key, new Map());
	}

	if (end <= start || partitions.length === 0) {
		return timelines;
	}

	for (const row of rows) {
		const partitionRows = rowsByPartition.get(getPartitionKey(row));
		if (partitionRows && row.bucketStart >= start && row.bucketStart < end) {
			partitionRows.set(row.bucketStart, row);
		}
	}

	const sourceToPartitions = new Map<string, string[]>();
	const sourceIds = new Set<string>();
	for (const partition of partitions) {
		for (const sourceId of partition.sourceIds) {
			sourceIds.add(sourceId);
			const partitionKeys = sourceToPartitions.get(sourceId) ?? [];
			partitionKeys.push(partition.key);
			sourceToPartitions.set(sourceId, partitionKeys);
		}
	}

	if (sourceIds.size > 0) {
		const sourceIdList = [...sourceIds];
		const coverageRows = await db.all<CoverageRow>(
			`
				SELECT
					source_id AS sourceId,
					bucket_start AS bucketStart,
					bucket_end AS bucketEnd,
					coverage_state AS coverageState,
					observed_units AS observedUnits,
					expected_units AS expectedUnits,
					rejected_units AS rejectedUnits
				FROM bucket_coverage
				WHERE granularity = ?
					AND source_id IN (${sourceIdList.map(() => '?').join(',')})
					AND bucket_start >= ?
					AND bucket_start < ?
				ORDER BY source_id ASC, bucket_start ASC
			`,
			[granularity, ...sourceIdList, start, end]
		);

		for (const row of coverageRows) {
			const partitionKeys = sourceToPartitions.get(row.sourceId) ?? [];
			for (const partitionKey of partitionKeys) {
				const partitionCoverage = coverageByPartition.get(partitionKey);
				if (!partitionCoverage) continue;
				const current = partitionCoverage.get(row.bucketStart);
				const normalizedState = normalizeCoverageState(row.coverageState);
				const aggregate = current ?? {
					bucketEnd: row.bucketEnd,
					state: normalizedState,
					observedUnits: 0,
					expectedUnits: 0,
					rejectedUnits: 0,
					hasPartialRow: false,
					hasUnknownRow: false,
					hasCompleteRow: false
				};
				aggregate.observedUnits += row.observedUnits;
				aggregate.expectedUnits += row.expectedUnits;
				aggregate.rejectedUnits += row.rejectedUnits;
				aggregate.bucketEnd = Math.min(aggregate.bucketEnd, row.bucketEnd);
				aggregate.hasPartialRow ||= normalizedState === 'partial';
				aggregate.hasUnknownRow ||= normalizedState === 'unknown';
				aggregate.hasCompleteRow ||= normalizedState === 'complete';
				aggregate.state = aggregateCoverageState(aggregate);
				partitionCoverage.set(row.bucketStart, aggregate);
			}
		}
	}

	for (const partition of partitions) {
		const timeline = timelines.get(partition.key);
		const partitionRows = rowsByPartition.get(partition.key);
		const partitionCoverage = coverageByPartition.get(partition.key);
		if (!timeline || !partitionRows || !partitionCoverage) continue;

		let bucketStart = start;
		while (bucketStart < end) {
			const storedCoverage = partitionCoverage.get(bucketStart);
			const bucketEnd = Math.min(
				storedCoverage?.bucketEnd ?? nextBucketEnd(bucketStart, granularity),
				end
			);
			const coverage = storedCoverage ?? unknownCoverage();
			const row = partitionRows.get(bucketStart);
			const data =
				coverage.state === 'unknown'
					? null
					: row
						? toData(row)
						: coverage.state === 'complete'
							? emptyData(partition.key)
							: null;

			timeline.push({
				bucketStart,
				bucketEnd,
				coverage: {
					state: coverage.state,
					observedUnits: coverage.observedUnits,
					expectedUnits: coverage.expectedUnits
				},
				data
			});
			bucketStart = bucketEnd;
		}
	}

	return timelines;
}

/**
 * Build source-separated coverage timelines without carrying a metric payload.
 * This shares bucket aggregation and civil-day handling with metric routes.
 */
export async function buildCoverageOnlyTimelines(
	options: BuildCoverageOnlyTimelinesOptions
): Promise<CoverageTimeline[]> {
	const { db, granularity, start, end, sourceIds } = options;
	const timelines = await buildCoverageTimelines<null, TimelineRow>({
		db,
		granularity,
		start,
		end,
		partitions: sourceIds.map((sourceId) => ({ key: sourceId, sourceIds: [sourceId] })),
		rows: [],
		getPartitionKey: () => '',
		toData: () => null,
		emptyData: () => null
	});

	return sourceIds.map((sourceId) => ({
		sourceId,
		buckets: (timelines.get(sourceId) ?? []).map(
			({ bucketStart, bucketEnd, coverage }): CoverageTimeline['buckets'][number] => ({
				bucketStart,
				bucketEnd,
				coverage
			})
		)
	}));
}

function nextBucketEnd(bucketStart: number, granularity: IpGranularity): number {
	if (granularity !== '1d') {
		return bucketStart + GRANULARITY_SECONDS[granularity];
	}

	const current = epochToPSTComponents(bucketStart);
	const nextDate = new Date(Date.UTC(current.year, current.month - 1, current.day + 1));
	return Math.floor(
		createDateFromPSTComponents(
			nextDate.getUTCFullYear(),
			nextDate.getUTCMonth() + 1,
			nextDate.getUTCDate()
		).getTime() / 1000
	);
}

function normalizeCoverageState(state: string): CoverageState {
	return state === 'complete' || state === 'partial' || state === 'unknown' ? state : 'unknown';
}

function aggregateCoverageState(coverage: CoverageAggregate): CoverageState {
	if (coverage.hasPartialRow || coverage.rejectedUnits > 0) return 'partial';
	if (coverage.observedUnits === 0) return 'unknown';
	if (coverage.hasUnknownRow || coverage.hasCompleteRow === false) return 'partial';
	return coverage.observedUnits === coverage.expectedUnits ? 'complete' : 'partial';
}

function unknownCoverage(): BucketCoverage {
	return { state: 'unknown', observedUnits: 0, expectedUnits: 0 };
}
