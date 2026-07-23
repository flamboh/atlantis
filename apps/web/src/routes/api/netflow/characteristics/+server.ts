import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import type {
	FlowCharacteristicsResponse,
	ObservationStatsBucket,
	PortCardinalityBucket
} from '$lib/types/types';
import {
	getDatasetDb,
	getRequestedDataset,
	listDatasetSourceDefinitions
} from '$lib/server/datasets';
import { parseAggregateStatsParams, placeholders, resolveSourceIds } from '$lib/server/netflow-v3';

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

function average(sum: number, count: number): number | null {
	return count === 0 ? null : sum / count;
}

function toObservationBucket(row: ObservationTotalsRow): ObservationStatsBucket {
	return {
		bucketStart: row.bucketStart,
		bucketEnd: row.bucketEnd,
		ipFamily: row.ipVersion === 4 ? 'ipv4' : 'ipv6',
		averageDurationMs: average(row.durationSumMs, row.durationCount),
		averageMinTtl: average(row.minTtlSum, row.minTtlCount),
		averageMaxTtl: average(row.maxTtlSum, row.maxTtlCount)
	};
}

function mergeIpFamilies(rows: ObservationTotalsRow[]): ObservationStatsBucket[] {
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
		...toObservationBucket(row),
		ipFamily: 'all'
	}));
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
		const portRows = await db.all<PortCardinalityBucket & { ipVersion: 4 | 6 }>(
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

		const response: FlowCharacteristicsResponse = {
			observationBuckets: [
				...observationRows.map(toObservationBucket),
				...mergeIpFamilies(observationRows)
			].sort((left, right) => left.bucketStart - right.bucketStart),
			portBuckets: portRows.map(({ ipVersion, ...row }) => ({
				...row,
				ipFamily: ipVersion === 4 ? 'ipv4' : 'ipv6'
			})),
			resolvedSources
		};
		return json(response);
	} catch (error) {
		console.error('Failed to query flow characteristics:', error);
		return json({ error: 'Database query failed' }, { status: 500 });
	}
};
