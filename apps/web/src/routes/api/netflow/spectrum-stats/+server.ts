import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import type { SpectrumPoint, SpectrumStatsBucket, SpectrumStatsResponse } from '$lib/types/types';
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
		const tableName = 'spectrum_stats_v2';
		const sourceColumn = 'source_id';
		const params = [granularity, ...routers, start, end];

		const query = `
			SELECT
				${sourceColumn} AS router,
				bucket_start AS bucketStart,
				spectrum_json_sa AS spectrumJsonSa,
				spectrum_json_da AS spectrumJsonDa
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
			spectrumJsonSa: string;
			spectrumJsonDa: string;
		}>(query, params);
		const buckets: SpectrumStatsBucket[] = rows.map((row) => {
			let spectrumSa: SpectrumPoint[] = [];
			let spectrumDa: SpectrumPoint[] = [];

			try {
				if (row.spectrumJsonSa) {
					spectrumSa = JSON.parse(row.spectrumJsonSa) as SpectrumPoint[];
				}
			} catch (e) {
				console.error('Failed to parse spectrum_json_sa:', e);
			}

			try {
				if (row.spectrumJsonDa) {
					spectrumDa = JSON.parse(row.spectrumJsonDa) as SpectrumPoint[];
				}
			} catch (e) {
				console.error('Failed to parse spectrum_json_da:', e);
			}

			return {
				bucketStart: row.bucketStart,
				router: row.router,
				spectrumSa,
				spectrumDa
			};
		});

		const response: SpectrumStatsResponse = {
			buckets,
			requestedRouters: routers
		};

		return json(response);
	} catch (error) {
		console.error('Failed to query spectrum_stats:', error);
		return json({ error: 'Database query failed' }, { status: 500 });
	}
};
