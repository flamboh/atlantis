import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import {
	IP_GRANULARITIES,
	type ProtocolStatsBucket,
	type ProtocolStatsResponse
} from '$lib/types/types';
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
		const tableName = 'protocol_stats_v2';
		const sourceColumn = 'source_id';
		const params = [granularity, ...routers, start, end];

		const query = `
			SELECT
				${sourceColumn} AS router,
				bucket_start AS bucketStart,
				bucket_end   AS bucketEnd,
				granularity,
				SUM(unique_protocols_count_ipv4) AS uniqueProtocolsIpv4,
				SUM(unique_protocols_count_ipv6) AS uniqueProtocolsIpv6,
				MAX(processed_at) AS processedAt
			FROM ${tableName}
			WHERE granularity = ?
				AND ${sourceColumn} IN (${placeholders(routers)})
				AND bucket_start >= ?
				AND bucket_start < ?
			GROUP BY ${sourceColumn}, bucket_start, bucket_end, granularity
			ORDER BY ${sourceColumn} ASC, bucket_start ASC
		`;

		const rows = await db.all<ProtocolStatsBucket>(query, params);
		const response: ProtocolStatsResponse = {
			buckets: rows.map((row) => ({
				...row,
				granularity
			})),
			availableGranularities: [...IP_GRANULARITIES],
			requestedRouters: routers
		};

		return json(response);
	} catch (error) {
		console.error('Failed to query protocol_stats:', error);
		const message = error instanceof Error ? error.message : 'Database query failed';
		const status = message.startsWith('Unknown dataset') ? 400 : 500;
		return json({ error: message }, { status });
	}
};
