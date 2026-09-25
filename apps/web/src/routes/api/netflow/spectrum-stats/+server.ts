import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import type { SpectrumStatsPayload, SpectrumStatsResponse } from '$lib/types/spectrum-stats';
import { buildCoverageTimelines } from '$lib/server/db/coverage';
import { getRequestedDataset, withDatasetDb } from '$lib/server/datasets';
import {
	buildSpectrumPoints,
	parseMaadStatsParams,
	placeholders,
	spectrumMeasureError
} from '$lib/server/netflow-v3';

type SpectrumStatsRow = SpectrumStatsPayload & {
	router: string;
	bucketStart: number;
	bucketEnd: number;
};

type RawSpectrumStatsRow = {
	router: string;
	bucketStart: number;
	bucketEnd: number;
	saSpectrum: Uint8Array | null;
	daSpectrum: Uint8Array | null;
};

export const GET: RequestHandler = async ({ url, platform }) => {
	const params = parseMaadStatsParams(url);
	if ('error' in params) {
		return json({ error: params.error }, { status: params.status });
	}
	const measureError = spectrumMeasureError(params.measure);
	if (measureError) {
		return json({ error: measureError.error }, { status: measureError.status });
	}
	const { routers, granularity, start, end, srcLocality, dstLocality, ipVersion, measure } = params;

	try {
		const dataset = await getRequestedDataset(url, platform);
		return await withDatasetDb(dataset, platform, async ({ db }) => {
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
					MAX(CASE WHEN address_side = 'source' THEN spectrum END) AS saSpectrum,
					MAX(CASE WHEN address_side = 'destination' THEN spectrum END) AS daSpectrum
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

			const rawRows = await db.all<RawSpectrumStatsRow>(query, queryParams);
			const rows: SpectrumStatsRow[] = rawRows.map((row) => ({
				router: row.router,
				bucketStart: row.bucketStart,
				bucketEnd: row.bucketEnd,
				spectrumSa: buildSpectrumPoints(row.saSpectrum) ?? [],
				spectrumDa: buildSpectrumPoints(row.daSpectrum) ?? []
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
