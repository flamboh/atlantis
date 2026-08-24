import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import type {
	FlowCharacteristicsResponse,
	ObservationStats,
	PortCardinalityCounts
} from '$lib/types/types';
import { getRequestedDataset, withDatasetDb } from '$lib/server/datasets';
import { parseAggregateStatsParams, placeholders, resolveSourceIds } from '$lib/server/netflow-v3';
import { buildCoverageTimelines, loadCoverageRows } from '$lib/server/db/coverage';

type ObservationTotalsRow = {
	bucketStart: number;
	bucketEnd: number;
	ipVersion: 4 | 6;
	durationSumMs: number;
	durationCount: number;
	minTtlSum: number;
	minTtlCount: number;
	maxTtlSum: number;
	maxTtlCount: number;
};

type TimedObservationStats = ObservationStats & { bucketStart: number };

type PortCardinalityRow = {
	sourceId: string;
	bucketStart: number;
	ipv4SourceLow: number;
	ipv4SourceHigh: number;
	ipv4DestinationLow: number;
	ipv4DestinationHigh: number;
	ipv6SourceLow: number;
	ipv6SourceHigh: number;
	ipv6DestinationLow: number;
	ipv6DestinationHigh: number;
};

type GroupedObservationRow = {
	bucketStart: number;
	values: ObservationStats[];
};

type GroupedPortRow = {
	sourceId: string;
	bucketStart: number;
	values: PortCardinalityCounts;
};

function emptyPortCounts(): PortCardinalityCounts {
	return {
		ipv4: {
			source: { low: 0, high: 0 },
			destination: { low: 0, high: 0 }
		},
		ipv6: {
			source: { low: 0, high: 0 },
			destination: { low: 0, high: 0 }
		}
	};
}

function average(sum: number, count: number): number | null {
	return count === 0 ? null : sum / count;
}

function toObservationStats(row: ObservationTotalsRow): TimedObservationStats {
	return {
		bucketStart: row.bucketStart,
		ipFamily: row.ipVersion === 4 ? 'ipv4' : 'ipv6',
		averageDurationMs: average(row.durationSumMs, row.durationCount),
		averageMinTtl: average(row.minTtlSum, row.minTtlCount),
		averageMaxTtl: average(row.maxTtlSum, row.maxTtlCount)
	};
}

function mergeIpFamilies(rows: ObservationTotalsRow[]): TimedObservationStats[] {
	const totalsByBucket = new Map<number, ObservationTotalsRow>();
	for (const row of rows) {
		const current = totalsByBucket.get(row.bucketStart);
		if (!current) {
			totalsByBucket.set(row.bucketStart, { ...row });
			continue;
		}
		current.bucketEnd = Math.max(current.bucketEnd, row.bucketEnd);
		current.durationSumMs += row.durationSumMs;
		current.durationCount += row.durationCount;
		current.minTtlSum += row.minTtlSum;
		current.minTtlCount += row.minTtlCount;
		current.maxTtlSum += row.maxTtlSum;
		current.maxTtlCount += row.maxTtlCount;
	}

	return [...totalsByBucket.values()].map((row) => ({
		...toObservationStats(row),
		ipFamily: 'all'
	}));
}

function groupObservations(rows: TimedObservationStats[]): GroupedObservationRow[] {
	const valuesByBucket = new Map<number, ObservationStats[]>();
	for (const { bucketStart, ...value } of rows) {
		const values = valuesByBucket.get(bucketStart) ?? [];
		values.push(value);
		valuesByBucket.set(bucketStart, values);
	}
	return [...valuesByBucket].map(([bucketStart, values]) => ({ bucketStart, values }));
}

function groupPorts(rows: PortCardinalityRow[]): GroupedPortRow[] {
	return rows.map((row) => ({
		sourceId: row.sourceId,
		bucketStart: row.bucketStart,
		values: {
			ipv4: {
				source: { low: row.ipv4SourceLow, high: row.ipv4SourceHigh },
				destination: { low: row.ipv4DestinationLow, high: row.ipv4DestinationHigh }
			},
			ipv6: {
				source: { low: row.ipv6SourceLow, high: row.ipv6SourceHigh },
				destination: { low: row.ipv6DestinationLow, high: row.ipv6DestinationHigh }
			}
		}
	}));
}

export const GET: RequestHandler = async ({ url, platform }) => {
	const params = parseAggregateStatsParams(url);
	if ('error' in params) {
		return json({ error: params.error }, { status: params.status });
	}

	try {
		const dataset = await getRequestedDataset(url, platform);
		return await withDatasetDb(dataset, platform, async ({ db, listSourceDefinitions }) => {
			const resolvedSources = resolveSourceIds(await listSourceDefinitions(), params.routers);
			const commonParams = [
				...resolvedSources,
				params.granularity,
				params.srcVisibility,
				params.dstVisibility,
				params.start,
				params.end
			];
			const sourcePlaceholders = placeholders(resolvedSources);
			const observationRowsPromise = db.all<ObservationTotalsRow>(
				`
				SELECT
					bucket_start AS bucketStart,
					MAX(bucket_end) AS bucketEnd,
					ip_version AS ipVersion,
					SUM(duration_sum_ms) AS durationSumMs,
					SUM(duration_count) AS durationCount,
					SUM(min_ttl_sum) AS minTtlSum,
					SUM(min_ttl_count) AS minTtlCount,
					SUM(max_ttl_sum) AS maxTtlSum,
					SUM(max_ttl_count) AS maxTtlCount
				FROM traffic_stats
				WHERE source_id IN (${sourcePlaceholders})
					AND granularity = ?
					AND src_visibility = ?
					AND dst_visibility = ?
					AND bucket_start >= ?
					AND bucket_start < ?
				GROUP BY +bucket_start, +ip_version
				ORDER BY +bucket_start, +ip_version
			`,
				commonParams
			);
			const portRowsPromise = db.all<PortCardinalityRow>(
				`
				SELECT
					source_id AS sourceId,
					bucket_start AS bucketStart,
					SUM(CASE WHEN ip_version = 4 AND port_side = 'source' AND port_range = 'low' THEN unique_port_count ELSE 0 END) AS ipv4SourceLow,
					SUM(CASE WHEN ip_version = 4 AND port_side = 'source' AND port_range = 'high' THEN unique_port_count ELSE 0 END) AS ipv4SourceHigh,
					SUM(CASE WHEN ip_version = 4 AND port_side = 'destination' AND port_range = 'low' THEN unique_port_count ELSE 0 END) AS ipv4DestinationLow,
					SUM(CASE WHEN ip_version = 4 AND port_side = 'destination' AND port_range = 'high' THEN unique_port_count ELSE 0 END) AS ipv4DestinationHigh,
					SUM(CASE WHEN ip_version = 6 AND port_side = 'source' AND port_range = 'low' THEN unique_port_count ELSE 0 END) AS ipv6SourceLow,
					SUM(CASE WHEN ip_version = 6 AND port_side = 'source' AND port_range = 'high' THEN unique_port_count ELSE 0 END) AS ipv6SourceHigh,
					SUM(CASE WHEN ip_version = 6 AND port_side = 'destination' AND port_range = 'low' THEN unique_port_count ELSE 0 END) AS ipv6DestinationLow,
					SUM(CASE WHEN ip_version = 6 AND port_side = 'destination' AND port_range = 'high' THEN unique_port_count ELSE 0 END) AS ipv6DestinationHigh
				FROM port_count_stats
				WHERE source_id IN (${sourcePlaceholders})
					AND granularity = ?
					AND src_visibility = ?
					AND dst_visibility = ?
					AND bucket_start >= ?
					AND bucket_start < ?
				GROUP BY source_id, bucket_start
			`,
				commonParams
			);
			const coverageRowsPromise = loadCoverageRows({
				db,
				granularity: params.granularity,
				start: params.start,
				end: params.end,
				sourceIds: resolvedSources
			});
			const [observationRows, portRows, coverageRows] = await Promise.all([
				observationRowsPromise,
				portRowsPromise,
				coverageRowsPromise
			]);

			const observationTimelines = await buildCoverageTimelines({
				db,
				granularity: params.granularity,
				start: params.start,
				end: params.end,
				partitions: [{ key: 'observations', sourceIds: resolvedSources }],
				rows: groupObservations([
					...observationRows.map(toObservationStats),
					...mergeIpFamilies(observationRows)
				]),
				getPartitionKey: () => 'observations',
				toData: (row) => row.values,
				emptyData: () => [],
				coverageRows
			});
			const portTimelines = await buildCoverageTimelines({
				db,
				granularity: params.granularity,
				start: params.start,
				end: params.end,
				partitions: resolvedSources.map((sourceId) => ({ key: sourceId, sourceIds: [sourceId] })),
				rows: groupPorts(portRows),
				getPartitionKey: (row) => row.sourceId,
				toData: (row) => row.values,
				emptyData: emptyPortCounts,
				coverageRows
			});

			const response: FlowCharacteristicsResponse = {
				observationBuckets: observationTimelines.get('observations') ?? [],
				portTimelines: resolvedSources.map((sourceId) => ({
					sourceId,
					buckets: portTimelines.get(sourceId) ?? []
				})),
				resolvedSources
			};
			return json(response);
		});
	} catch (error) {
		console.error('Failed to query flow characteristics:', error);
		return json({ error: 'Database query failed' }, { status: 500 });
	}
};
