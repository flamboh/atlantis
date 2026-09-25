import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import type { StructureStatsPayload, StructureStatsResponse } from '$lib/types/structure-stats';
import { buildCoverageTimelines } from '$lib/server/db/coverage';
import { getRequestedDataset, withDatasetDb } from '$lib/server/datasets';
import {
	buildStructurePoints,
	getMaadQGrid,
	parseMaadStatsParams,
	placeholders
} from '$lib/server/netflow-v3';

type StructureStatsRow = StructureStatsPayload & {
	router: string;
	bucketStart: number;
	bucketEnd: number;
};

type RawStructureStatsRow = {
	router: string;
	bucketStart: number;
	bucketEnd: number;
	saTau: Uint8Array | null;
	saTauSd: Uint8Array | null;
	daTau: Uint8Array | null;
	daTauSd: Uint8Array | null;
};

export const GET: RequestHandler = async ({ url, platform }) => {
	const params = parseMaadStatsParams(url);
	if ('error' in params) {
		return json({ error: params.error }, { status: params.status });
	}
	const { routers, granularity, start, end, srcLocality, dstLocality, ipVersion, measure } = params;

	try {
		const dataset = await getRequestedDataset(url, platform);
		return await withDatasetDb(dataset, platform, async ({ db }) => {
			const qGrid = await getMaadQGrid(db, ipVersion);

			const tableName = 'address_maad_stats';
			const sourceColumn = 'source_id';
			const queryParams = [
				granularity,
				...routers,
				srcLocality,
				dstLocality,
				start,
				end,
				ipVersion,
				measure
			];

			const query = `
				SELECT
					${sourceColumn} AS router,
					bucket_start AS bucketStart,
					MAX(bucket_end) AS bucketEnd,
					MAX(CASE WHEN address_side = 'source' THEN tau END) AS saTau,
					MAX(CASE WHEN address_side = 'source' THEN tau_sd END) AS saTauSd,
					MAX(CASE WHEN address_side = 'destination' THEN tau END) AS daTau,
					MAX(CASE WHEN address_side = 'destination' THEN tau_sd END) AS daTauSd
				FROM ${tableName}
				WHERE granularity = ?
					AND ${sourceColumn} IN (${placeholders(routers)})
					AND src_locality = ?
					AND dst_locality = ?
					AND bucket_start >= ?
					AND bucket_start < ?
					AND ip_version = ?
					AND measure = ?
				GROUP BY ${sourceColumn}, bucket_start
			`;

			const rawRows = await db.all<RawStructureStatsRow>(query, queryParams);
			const rows: StructureStatsRow[] = rawRows.map((row) => ({
				router: row.router,
				bucketStart: row.bucketStart,
				bucketEnd: row.bucketEnd,
				structureSa: buildStructurePoints(row.saTau, row.saTauSd, qGrid),
				structureDa: buildStructurePoints(row.daTau, row.daTauSd, qGrid)
			}));
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

			const response: StructureStatsResponse = {
				timelines: routers.map((router) => ({
					router,
					buckets: timelines.get(router) ?? []
				})),
				requestedRouters: routers
			};

			return json(response);
		});
	} catch (error) {
		console.error('Failed to query structure_stats:', error);
		return json({ error: 'Database query failed' }, { status: 500 });
	}
};
