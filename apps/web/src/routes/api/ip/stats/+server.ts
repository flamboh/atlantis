import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import type { IpStatsBucket, IpStatsResponse } from '$lib/types/types';
import { buildCoverageTimelines } from '$lib/server/db/coverage';
import { getRequestedDataset, withDatasetDb } from '$lib/server/datasets';
import { parseAggregateStatsParams, placeholders } from '$lib/server/netflow-v3';

export const GET: RequestHandler = async ({ url, platform }) => {
	const params = parseAggregateStatsParams(url);
	if ('error' in params) {
		return json({ error: params.error }, { status: params.status });
	}
	const { routers, granularity, start, end, srcVisibility, dstVisibility } = params;

	try {
		const dataset = await getRequestedDataset(url, platform);
		return await withDatasetDb(dataset, platform, async ({ db }) => {
			const tableName = 'address_count_stats';
			const sourceColumn = 'source_id';
			const queryParams = [granularity, ...routers, srcVisibility, dstVisibility, start, end];

			const query = `
			SELECT
				${sourceColumn} AS router,
				bucket_start AS bucketStart,
				SUM(CASE WHEN address_side = 'source' AND ip_version = 4 THEN unique_address_count ELSE 0 END) AS saIpv4Count,
				SUM(CASE WHEN address_side = 'destination' AND ip_version = 4 THEN unique_address_count ELSE 0 END) AS daIpv4Count,
				SUM(CASE WHEN address_side = 'source' AND ip_version = 6 THEN unique_address_count ELSE 0 END) AS saIpv6Count,
				SUM(CASE WHEN address_side = 'destination' AND ip_version = 6 THEN unique_address_count ELSE 0 END) AS daIpv6Count
			FROM ${tableName}
			WHERE granularity = ?
				AND ${sourceColumn} IN (${placeholders(routers)})
				AND src_visibility = ?
				AND dst_visibility = ?
				AND bucket_start >= ?
				AND bucket_start < ?
			GROUP BY ${sourceColumn}, bucket_start
			ORDER BY ${sourceColumn} ASC, bucket_start ASC
		`;

			const rows = await db.all<IpStatsBucket & { router: string; bucketStart: number }>(
				query,
				queryParams
			);
			const timelines = await buildCoverageTimelines({
				db,
				granularity,
				start,
				end,
				partitions: routers.map((router) => ({ key: router, sourceIds: [router] })),
				rows,
				getPartitionKey: (row) => row.router,
				toData: ({ saIpv4Count, daIpv4Count, saIpv6Count, daIpv6Count }) => ({
					saIpv4Count,
					daIpv4Count,
					saIpv6Count,
					daIpv6Count
				}),
				emptyData: () => ({
					saIpv4Count: 0,
					daIpv4Count: 0,
					saIpv6Count: 0,
					daIpv6Count: 0
				})
			});
			const response: IpStatsResponse = {
				timelines: routers.map((router) => ({
					router,
					buckets: timelines.get(router) ?? []
				}))
			};

			return json(response);
		});
	} catch (error) {
		console.error('Failed to query ip_stats:', error);
		return json({ error: 'Database query failed' }, { status: 500 });
	}
};
