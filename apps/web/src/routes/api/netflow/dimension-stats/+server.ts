import type { RequestHandler } from './$types';
import type { DimensionStatsPayload, DimensionStatsResponse } from '$lib/types/dimension-stats';
import { buildCoverageTimelines } from '$lib/server/db/coverage';
import { getRequestedDataset, withDatasetDb } from '$lib/server/datasets';
import { parseMaadStatsParams, placeholders } from '$lib/server/netflow-v3';

type DimensionStatsRow = DimensionStatsPayload & {
	router: string;
	bucketStart: number;
	bucketEnd: number;
};

export const GET: RequestHandler = async ({ url, platform }) => {
	const params = parseMaadStatsParams(url);
	if ('error' in params) {
		return Response.json({ error: params.error }, { status: params.status });
	}
	const { routers, granularity, start, end, srcLocality, dstLocality, ipVersion, measure } = params;

	try {
		const dataset = await getRequestedDataset(url, platform);
		return await withDatasetDb(dataset, platform, async ({ db }) => {
			const rows = await db.all<DimensionStatsRow>(
				`
				SELECT
					source_id AS router,
					bucket_start AS bucketStart,
					MAX(bucket_end) AS bucketEnd,
					MAX(CASE WHEN address_side = 'source' THEN d0 END) AS saD0,
					MAX(CASE WHEN address_side = 'source' THEN d1 END) AS saD1,
					MAX(CASE WHEN address_side = 'source' THEN d2 END) AS saD2,
					MAX(CASE WHEN address_side = 'destination' THEN d0 END) AS daD0,
					MAX(CASE WHEN address_side = 'destination' THEN d1 END) AS daD1,
					MAX(CASE WHEN address_side = 'destination' THEN d2 END) AS daD2
				FROM address_maad_stats
				WHERE granularity = ?
					AND source_id IN (${placeholders(routers)})
					AND src_locality = ?
					AND dst_locality = ?
					AND bucket_start >= ?
					AND bucket_start < ?
					AND ip_version = ?
					AND measure = ?
				GROUP BY source_id, bucket_start
			`,
				[granularity, ...routers, srcLocality, dstLocality, start, end, ipVersion, measure]
			);
			const timelines = await buildCoverageTimelines({
				db,
				granularity,
				start,
				end,
				partitions: routers.map((router) => ({ key: router, sourceIds: [router] })),
				rows,
				getPartitionKey: (row) => row.router,
				toData: ({ router: _router, bucketStart: _bucketStart, bucketEnd: _bucketEnd, ...data }) =>
					data,
				emptyData: () => null
			});

			const response: DimensionStatsResponse = {
				timelines: routers.map((router) => ({
					router,
					buckets: timelines.get(router) ?? []
				})),
				requestedRouters: routers
			};

			return Response.json(response);
		});
	} catch (error) {
		console.error('Failed to query MAAD dimensions:', error);
		return Response.json({ error: 'Database query failed' }, { status: 500 });
	}
};
