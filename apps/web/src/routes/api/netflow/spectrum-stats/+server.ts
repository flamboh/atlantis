import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import type { SpectrumPoint } from '$lib/types/types';
import type { SpectrumStatsPayload, SpectrumStatsResponse } from '$lib/types/spectrum-stats';
import { buildCoverageTimelines } from '$lib/server/db/coverage';
import { getRequestedDataset, withDatasetDb } from '$lib/server/datasets';
import { parseAggregateStatsParams, placeholders } from '$lib/server/netflow-v3';

type SpectrumStatsRow = SpectrumStatsPayload & {
	router: string;
	bucketStart: number;
	bucketEnd: number;
};

type RawSpectrumStatsRow = {
	router: string;
	bucketStart: number;
	bucketEnd: number;
	spectrumSaJson: string | null;
	spectrumDaJson: string | null;
};

function isSpectrumPoint(value: unknown): value is SpectrumPoint {
	return (
		typeof value === 'object' &&
		value !== null &&
		'alpha' in value &&
		'f' in value &&
		typeof value.alpha === 'number' &&
		typeof value.f === 'number'
	);
}

function parseSpectrumPoints(
	valuesJson: string | null,
	router: string,
	bucketStart: number
): SpectrumPoint[] | null {
	if (valuesJson === null) return null;

	try {
		const values: unknown = JSON.parse(valuesJson);
		if (!Array.isArray(values)) return null;
		return values.filter(isSpectrumPoint);
	} catch (error) {
		console.error('Failed to parse spectrum values_json:', { router, bucketStart, error });
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
		return await withDatasetDb(dataset, platform, async ({ db }) => {
			const tableName = 'address_structure_stats';
			const sourceColumn = 'source_id';
			const queryParams = [granularity, ...routers, srcVisibility, dstVisibility, start, end];

			const query = `
			SELECT
				${sourceColumn} AS router,
				bucket_start AS bucketStart,
				MAX(bucket_end) AS bucketEnd,
				MAX(CASE WHEN address_side = 'source' THEN values_json END) AS spectrumSaJson,
				MAX(CASE WHEN address_side = 'destination' THEN values_json END) AS spectrumDaJson
			FROM ${tableName}
			WHERE granularity = ?
				AND ${sourceColumn} IN (${placeholders(routers)})
				AND src_visibility = ?
				AND dst_visibility = ?
				AND bucket_start >= ?
				AND bucket_start < ?
				AND ip_version = 4
				AND structure_kind = 'spectrum'
			GROUP BY ${sourceColumn}, bucket_start
		`;

			const rawRows = await db.all<RawSpectrumStatsRow>(query, queryParams);
			const rows: SpectrumStatsRow[] = rawRows.flatMap((row) => {
				const spectrumSa = parseSpectrumPoints(row.spectrumSaJson, row.router, row.bucketStart);
				const spectrumDa = parseSpectrumPoints(row.spectrumDaJson, row.router, row.bucketStart);
				return spectrumSa === null && spectrumDa === null
					? []
					: [
							{
								router: row.router,
								bucketStart: row.bucketStart,
								bucketEnd: row.bucketEnd,
								spectrumSa: spectrumSa ?? [],
								spectrumDa: spectrumDa ?? []
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

			const response: SpectrumStatsResponse = {
				timelines: routers.map((router) => ({
					router,
					buckets: timelines.get(router) ?? []
				})),
				requestedRouters: routers
			};

			return json(response);
		});
	} catch (error) {
		console.error('Failed to query spectrum_stats:', error);
		return json({ error: 'Database query failed' }, { status: 500 });
	}
};
