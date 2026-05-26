import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { IP_GRANULARITIES } from '$lib/types/types';
import type { IpStatsBucket, IpStatsResponse } from '$lib/types/types';
import { getDatasetDb, getRequestedDataset } from '$lib/server/datasets';
import { parseAggregateStatsParams, placeholders } from '$lib/server/netflow-v2';

export const GET: RequestHandler = async ({ url, platform }) => {
	const params = parseAggregateStatsParams(url);
	if ('error' in params) {
		return json({ error: params.error }, { status: params.status });
	}
	const { routers, granularity, start, end } = params;

	try {
		const dataset = await getRequestedDataset(url, platform);
		const db = await getDatasetDb(dataset, platform);
		const tableName = 'ip_stats_v2';
		const sourceColumn = 'source_id';
		const params = [granularity, ...routers, start, end];

		const query = `
			SELECT
				${sourceColumn} AS router,
				bucket_start AS bucketStart,
				bucket_end   AS bucketEnd,
				granularity,
				SUM(sa_ipv4_count) AS saIpv4Count,
				SUM(da_ipv4_count) AS daIpv4Count,
				SUM(sa_ipv6_count) AS saIpv6Count,
				SUM(da_ipv6_count) AS daIpv6Count,
				MAX(processed_at) AS processedAt
			FROM ${tableName}
			WHERE granularity = ?
				AND ${sourceColumn} IN (${placeholders(routers)})
				AND bucket_start >= ?
				AND bucket_start < ?
			GROUP BY ${sourceColumn}, bucket_start, bucket_end, granularity
			ORDER BY ${sourceColumn} ASC, bucket_start ASC
		`;

		const rows = await db.all<IpStatsBucket>(query, params);
		const response: IpStatsResponse = {
			buckets: rows.map((row) => ({
				...row,
				granularity
			})),
			availableGranularities: [...IP_GRANULARITIES],
			requestedRouters: routers
		};

		return json(response);
	} catch (error) {
		console.error('Failed to query ip_stats:', error);
		return json({ error: 'Database query failed' }, { status: 500 });
	}
};
