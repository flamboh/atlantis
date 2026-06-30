import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import type { SpectrumData, SpectrumPoint } from '$lib/types/types';
import { parseFlowScopeParams } from '$lib/server/netflow-v3';
import { getDatasetFromRequest, getDb, slugToBucketStart } from '../utils';

const FIVE_MINUTES = '5m';

type SpectrumRow = {
	valuesJson: string | null;
};

export const GET: RequestHandler = async ({ params, url, platform }) => {
	const { slug } = params;
	const dataset = await getDatasetFromRequest(url, platform);
	const router = url.searchParams.get('router');
	const sourceParam = url.searchParams.get('source');
	const flowScope = parseFlowScopeParams(url);

	if ('error' in flowScope) {
		return json({ error: flowScope.error }, { status: flowScope.status });
	}

	if (!slug || slug.length !== 12 || !/^\d{12}$/.test(slug)) {
		return json({ error: 'Invalid slug format' }, { status: 400 });
	}

	if (!router) {
		return json({ error: 'Router parameter is required' }, { status: 400 });
	}

	if (sourceParam === null) {
		return json(
			{ error: 'Source parameter is required (true for source addresses, false for destination)' },
			{ status: 400 }
		);
	}

	const isSource = sourceParam === 'true';
	const bucketStart = slugToBucketStart(slug);

	if (bucketStart === null) {
		return json({ error: 'Unable to parse slug timestamp' }, { status: 400 });
	}

	try {
		const db = await getDb(dataset, platform);
		const row = await db.get<SpectrumRow>(
			`SELECT
				values_json AS valuesJson
			FROM address_structure_stats
			WHERE source_id = ?
				AND granularity = ?
				AND bucket_start = ?
				AND ip_version = 4
				AND src_visibility = ?
				AND dst_visibility = ?
				AND address_side = ?
				AND structure_kind = 'spectrum'
			LIMIT 1`,
			[
				router,
				FIVE_MINUTES,
				bucketStart,
				flowScope.srcVisibility,
				flowScope.dstVisibility,
				isSource ? 'source' : 'destination'
			]
		);

		if (!row) {
			return json(
				{ error: `Spectrum statistics not found for router ${router} at ${slug}` },
				{ status: 404 }
			);
		}

		const rawSpectrum = row.valuesJson;
		if (!rawSpectrum) {
			return json(
				{ error: `Spectrum statistics not found for router ${router} at ${slug}` },
				{ status: 404 }
			);
		}

		let data: SpectrumPoint[] = [];

		try {
			data = JSON.parse(rawSpectrum) as SpectrumPoint[];
		} catch (error) {
			console.error('Failed to parse spectrum JSON from database:', error);
			return json({ error: 'Failed to parse spectrum statistics' }, { status: 500 });
		}

		if (data.length === 0) {
			return json(
				{ error: `Spectrum statistics not found for router ${router} at ${slug}` },
				{ status: 404 }
			);
		}

		const addressType = isSource ? 'Source' : 'Destination';
		const alphaValues = data.map((point) => point.alpha);
		const alphaRange =
			alphaValues.length > 0
				? { min: Math.min(...alphaValues), max: Math.max(...alphaValues) }
				: { min: 0, max: 0 };

		const response: SpectrumData = {
			slug,
			router,
			filename: `nfcapd.${slug}`,
			spectrum: data,
			metadata: {
				dataSource: `Database: spectrum_stats 5m bucket (${addressType} Addresses)`,
				uniqueIPCount: -1,
				pointCount: data.length,
				addressType: addressType,
				alphaRange
			}
		};

		return json(response);
	} catch (error) {
		console.error('Failed to fetch spectrum statistics from database:', error);
		return json({ error: 'Failed to get spectrum statistics' }, { status: 500 });
	}
};
