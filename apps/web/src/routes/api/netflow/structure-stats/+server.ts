import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import type { StructureFunctionPoint } from '$lib/types/types';
import type { StructureStatsPayload, StructureStatsResponse } from '$lib/types/structure-stats';
import { buildCoverageTimelines } from '$lib/server/db/coverage';
import { getDatasetDb, getRequestedDataset } from '$lib/server/datasets';
import {
	normalizeStructurePoints,
	parseAggregateStatsParams,
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
	structureSaJson: string | null;
	structureDaJson: string | null;
};

type RawStructurePoint = {
	q: number;
	tau?: number;
	tauTilde?: number;
	sd?: number;
	s?: number;
};

function isRawStructurePoint(value: unknown): value is RawStructurePoint {
	return (
		typeof value === 'object' &&
		value !== null &&
		'q' in value &&
		typeof value.q === 'number' &&
		(!('tau' in value) || typeof value.tau === 'number') &&
		(!('tauTilde' in value) || typeof value.tauTilde === 'number') &&
		(!('sd' in value) || typeof value.sd === 'number') &&
		(!('s' in value) || typeof value.s === 'number')
	);
}

function parseStructurePoints(
	valuesJson: string | null,
	router: string,
	bucketStart: number
): StructureFunctionPoint[] | null {
	if (valuesJson === null) return null;

	try {
		const values: unknown = JSON.parse(valuesJson);
		if (!Array.isArray(values)) return null;
		return normalizeStructurePoints(values.filter(isRawStructurePoint));
	} catch (error) {
		console.error('Failed to parse structure values_json:', { router, bucketStart, error });
		return null;
	}
}

export const GET: RequestHandler = async ({ url, platform }) => {
	const params = parseAggregateStatsParams(url);
	if ('error' in params) {
		return json({ error: params.error }, { status: params.status });
	}
	const { routers, granularity, start, end, srcVisibility, dstVisibility } = params;

	try {
		const dataset = await getRequestedDataset(url, platform);
		const db = await getDatasetDb(dataset, platform);
		const tableName = 'address_structure_stats';
		const sourceColumn = 'source_id';
		const queryParams = [granularity, ...routers, srcVisibility, dstVisibility, start, end];

		const query = `
			SELECT
				${sourceColumn} AS router,
				bucket_start AS bucketStart,
				bucket_end AS bucketEnd,
				MAX(CASE WHEN address_side = 'source' THEN values_json END) AS structureSaJson,
				MAX(CASE WHEN address_side = 'destination' THEN values_json END) AS structureDaJson
			FROM ${tableName}
			WHERE granularity = ?
				AND ${sourceColumn} IN (${placeholders(routers)})
				AND src_visibility = ?
				AND dst_visibility = ?
				AND bucket_start >= ?
				AND bucket_start < ?
				AND ip_version = 4
				AND structure_kind = 'structure'
			GROUP BY ${sourceColumn}, bucket_start, bucket_end
			ORDER BY ${sourceColumn} ASC, bucket_start ASC
		`;

		const rawRows = await db.all<RawStructureStatsRow>(query, queryParams);
		const rows: StructureStatsRow[] = rawRows.flatMap((row) => {
			const structureSa = parseStructurePoints(row.structureSaJson, row.router, row.bucketStart);
			const structureDa = parseStructurePoints(row.structureDaJson, row.router, row.bucketStart);
			return structureSa === null && structureDa === null
				? []
				: [
						{
							router: row.router,
							bucketStart: row.bucketStart,
							bucketEnd: row.bucketEnd,
							structureSa: structureSa ?? [],
							structureDa: structureDa ?? []
						}
					];
		});
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
	} catch (error) {
		console.error('Failed to query structure_stats:', error);
		return json({ error: 'Database query failed' }, { status: 500 });
	}
};
