import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { IP_GRANULARITIES } from '$lib/types/types';
import type { IpStatsBucket, IpStatsResponse } from '$lib/types/types';
import { getDatasetDb, getRequestedDataset } from '$lib/server/datasets';
import { parseAggregateStatsParams, placeholders } from '$lib/server/netflow-v3';

export const GET: RequestHandler = async ({ url, platform }) => {
	const params = parseAggregateStatsParams(url);
	if ('error' in params) {
		return json({ error: params.error }, { status: params.status });
	}
	const { routers, granularity, start, end, srcVisibility, dstVisibility } = params;

	try {
		const dataset = await getRequestedDataset(url, platform);
		const db = await getDatasetDb(dataset, platform);
		const tableName = 'address_count_stats';
		const sourceColumn = 'source_id';
		const queryParams = [granularity, ...routers, srcVisibility, dstVisibility, start, end];

		const query = `
			SELECT
				${sourceColumn} AS router,
				bucket_start AS bucketStart,
				bucket_end   AS bucketEnd,
				granularity,
				SUM(CASE WHEN address_side = 'source' AND ip_version = 4 THEN unique_address_count ELSE 0 END) AS saIpv4Count,
				SUM(CASE WHEN address_side = 'destination' AND ip_version = 4 THEN unique_address_count ELSE 0 END) AS daIpv4Count,
				SUM(CASE WHEN address_side = 'source' AND ip_version = 6 THEN unique_address_count ELSE 0 END) AS saIpv6Count,
				SUM(CASE WHEN address_side = 'destination' AND ip_version = 6 THEN unique_address_count ELSE 0 END) AS daIpv6Count,
				MAX(processed_at) AS processedAt
			FROM ${tableName}
			WHERE granularity = ?
				AND ${sourceColumn} IN (${placeholders(routers)})
				AND src_visibility = ?
				AND dst_visibility = ?
				AND bucket_start >= ?
				AND bucket_start < ?
			GROUP BY ${sourceColumn}, bucket_start, bucket_end, granularity
			ORDER BY ${sourceColumn} ASC, bucket_start ASC
		`;

		const rows = await db.all<IpStatsBucket>(query, queryParams);
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
