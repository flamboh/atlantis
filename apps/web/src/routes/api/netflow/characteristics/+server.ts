import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import type {
	FlowCharacteristicsResponse,
	ObservationStats,
	PortCardinalityStats
} from '$lib/types/types';
import {
	getDatasetDb,
	getRequestedDataset,
	listDatasetSourceDefinitions
} from '$lib/server/datasets';
import { parseAggregateStatsParams, placeholders, resolveSourceIds } from '$lib/server/netflow-v3';
import { buildCoverageTimelines } from '$lib/server/db/coverage';

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
	bucketEnd: number;
	ipVersion: 4 | 6;
	portSide: PortCardinalityStats['portSide'];
	portRange: PortCardinalityStats['portRange'];
	uniquePortCount: number;
};

type GroupedObservationRow = {
	bucketStart: number;
	values: ObservationStats[];
};

type GroupedPortRow = {
	sourceId: string;
	bucketStart: number;
	values: PortCardinalityStats[];
};

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
	const valuesBySourceBucket = new Map<string, GroupedPortRow>();
	for (const { sourceId, bucketStart, ipVersion, bucketEnd: _bucketEnd, ...row } of rows) {
		const key = `${sourceId}\0${bucketStart}`;
		const grouped = valuesBySourceBucket.get(key) ?? { sourceId, bucketStart, values: [] };
		grouped.values.push({
			...row,
			ipFamily: ipVersion === 4 ? 'ipv4' : 'ipv6'
		});
		valuesBySourceBucket.set(key, grouped);
	}
	return [...valuesBySourceBucket.values()];
}

export const GET: RequestHandler = async ({ url, platform }) => {
	const params = parseAggregateStatsParams(url);
	if ('error' in params) {
		return json({ error: params.error }, { status: params.status });
	}

	try {
		const dataset = await getRequestedDataset(url, platform);
		const db = await getDatasetDb(dataset, platform);
		const resolvedSources = resolveSourceIds(
			await listDatasetSourceDefinitions(dataset, platform),
			params.routers
		);
		const commonParams = [
			...resolvedSources,
			params.granularity,
			params.srcVisibility,
			params.dstVisibility,
			params.start,
			params.end
		];
		const sourcePlaceholders = placeholders(resolvedSources);
		const observationRows = await db.all<ObservationTotalsRow>(
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
				GROUP BY bucket_start, ip_version
				ORDER BY bucket_start, ip_version
			`,
			commonParams
		);
		const portRows = await db.all<PortCardinalityRow>(
			`
				SELECT
					source_id AS sourceId,
					bucket_start AS bucketStart,
					bucket_end AS bucketEnd,
					ip_version AS ipVersion,
					port_side AS portSide,
					port_range AS portRange,
					unique_port_count AS uniquePortCount
				FROM port_count_stats
				WHERE source_id IN (${sourcePlaceholders})
					AND granularity = ?
					AND src_visibility = ?
					AND dst_visibility = ?
					AND bucket_start >= ?
					AND bucket_start < ?
				ORDER BY source_id, bucket_start, ip_version, port_side, port_range
			`,
			commonParams
		);

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
			emptyData: () => []
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
			emptyData: () => []
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
	} catch (error) {
		console.error('Failed to query flow characteristics:', error);
		return json({ error: 'Database query failed' }, { status: 500 });
	}
};
