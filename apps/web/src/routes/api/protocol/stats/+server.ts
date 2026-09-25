import type { RequestHandler } from './$types';
import type { ProtocolStatsBucket, ProtocolStatsResponse } from '#lib/types/types.ts';
import { buildCoverageTimelines } from '#lib/server/db/coverage.ts';
import { getRequestedDataset, withDatasetDb } from '#lib/server/datasets.ts';
import { parseAggregateStatsParams, placeholders } from '#lib/server/netflow-v3.ts';

export const GET: RequestHandler = async ({ url }) => {
	const params = parseAggregateStatsParams(url);
	if ('error' in params) {
		return Response.json({ error: params.error }, { status: params.status });
	}
	const { routers, granularity, start, end, srcVisibility, dstVisibility } = params;

	try {
		const dataset = await getRequestedDataset(url);
		return await withDatasetDb(dataset, async ({ db }) => {
			const tableName = 'protocol_stats';
			const sourceColumn = 'source_id';
			const queryParams = [granularity, ...routers, srcVisibility, dstVisibility, start, end];

			const query = `
			SELECT
				${sourceColumn} AS router,
				bucket_start AS bucketStart,
				SUM(CASE WHEN ip_version = 4 THEN unique_protocols_count ELSE 0 END) AS uniqueProtocolsIpv4,
				SUM(CASE WHEN ip_version = 6 THEN unique_protocols_count ELSE 0 END) AS uniqueProtocolsIpv6
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

			const rows = await db.all<ProtocolStatsBucket & { router: string; bucketStart: number }>(
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
				toData: ({ uniqueProtocolsIpv4, uniqueProtocolsIpv6 }) => ({
					uniqueProtocolsIpv4,
					uniqueProtocolsIpv6
				}),
				emptyData: () => ({
					uniqueProtocolsIpv4: 0,
					uniqueProtocolsIpv6: 0
				})
			});
			const response: ProtocolStatsResponse = {
				timelines: routers.map((router) => ({
					router,
					buckets: timelines.get(router) ?? []
				}))
			};

			return Response.json(response);
		});
	} catch (error) {
		console.error('Failed to query protocol_stats:', error);
		const message = error instanceof Error ? error.message : 'Database query failed';
		const status = message.startsWith('Unknown dataset') ? 400 : 500;
		return Response.json({ error: message }, { status });
	}
};
