import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import type {
	StructureFunctionPoint,
	StructureStatsBucket,
	StructureStatsResponse
} from '$lib/types/types';
import { getDatasetDb, getRequestedDataset } from '$lib/server/datasets';
import {
	normalizeStructurePoints,
	parseAggregateStatsParams,
	placeholders
} from '$lib/server/netflow-v2';

export const GET: RequestHandler = async ({ url, platform }) => {
	const params = parseAggregateStatsParams(url);
	if ('error' in params) {
		return json({ error: params.error }, { status: params.status });
	}
	const { routers, granularity, start, end } = params;

	try {
		const dataset = await getRequestedDataset(url, platform);
		const db = await getDatasetDb(dataset, platform);
		const tableName = 'structure_stats_v2';
		const sourceColumn = 'source_id';
		const params = [granularity, ...routers, start, end];

		const query = `
			SELECT
				${sourceColumn} AS router,
				bucket_start AS bucketStart,
				structure_json_sa AS structureJsonSa,
				structure_json_da AS structureJsonDa
			FROM ${tableName}
			WHERE granularity = ?
				AND ${sourceColumn} IN (${placeholders(routers)})
				AND bucket_start >= ?
				AND bucket_start < ?
				AND ip_version = 4
			ORDER BY ${sourceColumn} ASC, bucket_start ASC
		`;

		const rows = await db.all<{
			router: string;
			bucketStart: number;
			structureJsonSa: string;
			structureJsonDa: string;
		}>(query, params);
		const buckets: StructureStatsBucket[] = rows.map((row) => {
			let structureSa: StructureFunctionPoint[] = [];
			let structureDa: StructureFunctionPoint[] = [];

			try {
				if (row.structureJsonSa) {
					structureSa = normalizeStructurePoints(
						JSON.parse(row.structureJsonSa) as StructureFunctionPoint[]
					);
				}
			} catch (e) {
				console.error('Failed to parse structure_json_sa:', e);
			}

			try {
				if (row.structureJsonDa) {
					structureDa = normalizeStructurePoints(
						JSON.parse(row.structureJsonDa) as StructureFunctionPoint[]
					);
				}
			} catch (e) {
				console.error('Failed to parse structure_json_da:', e);
			}

			return {
				bucketStart: row.bucketStart,
				router: row.router,
				structureSa,
				structureDa
			};
		});

		const response: StructureStatsResponse = {
			buckets,
			requestedRouters: routers
		};

		return json(response);
	} catch (error) {
		console.error('Failed to query structure_stats:', error);
		return json({ error: 'Database query failed' }, { status: 500 });
	}
};
