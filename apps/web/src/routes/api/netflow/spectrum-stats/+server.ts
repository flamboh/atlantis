import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import type { SpectrumPoint, SpectrumStatsBucket, SpectrumStatsResponse } from '$lib/types/types';
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
		const tableName = 'address_structure_stats';
		const sourceColumn = 'source_id';
		const queryParams = [granularity, ...routers, srcVisibility, dstVisibility, start, end];

		const query = `
			SELECT
				${sourceColumn} AS router,
				bucket_start AS bucketStart,
				address_side AS addressSide,
				values_json AS valuesJson
			FROM ${tableName}
			WHERE granularity = ?
				AND ${sourceColumn} IN (${placeholders(routers)})
				AND src_visibility = ?
				AND dst_visibility = ?
				AND bucket_start >= ?
				AND bucket_start < ?
				AND ip_version = 4
				AND structure_kind = 'spectrum'
			ORDER BY ${sourceColumn} ASC, bucket_start ASC
		`;

		const rows = await db.all<{
			router: string;
			bucketStart: number;
			addressSide: 'source' | 'destination';
			valuesJson: string;
		}>(query, queryParams);
		const bucketsByKey = new Map<string, SpectrumStatsBucket>();

		for (const row of rows) {
			const key = `${row.router}:${row.bucketStart}`;
			const bucket =
				bucketsByKey.get(key) ??
				({
					bucketStart: row.bucketStart,
					router: row.router,
					spectrumSa: [],
					spectrumDa: []
				} satisfies SpectrumStatsBucket);
			let points: SpectrumPoint[] = [];
			try {
				points = JSON.parse(row.valuesJson) as SpectrumPoint[];
			} catch (e) {
				console.error('Failed to parse spectrum values_json:', e);
			}

			if (row.addressSide === 'source') {
				bucket.spectrumSa = points;
			} else {
				bucket.spectrumDa = points;
			}
			bucketsByKey.set(key, bucket);
		}

		const buckets = [...bucketsByKey.values()].sort(
			(left, right) =>
				left.router.localeCompare(right.router) || left.bucketStart - right.bucketStart
		);

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
