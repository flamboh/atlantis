import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { getDatasetFromRequest, getDb, slugToBucketStart } from '../utils';

const FIVE_MINUTES = '5m';
const DEFAULT_SRC_VISIBILITY = 'all';
const DEFAULT_DST_VISIBILITY = 'all';

type IpCountRow = {
	saIpv4Count: number;
	daIpv4Count: number;
	saIpv6Count: number;
	daIpv6Count: number;
};

export const GET: RequestHandler = async ({ params, url, platform }) => {
	const { slug } = params;
	const dataset = await getDatasetFromRequest(url, platform);
	const router = url.searchParams.get('router');
	const sourceParam = url.searchParams.get('source');

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
		const row = await db.get<IpCountRow>(
			`SELECT
				SUM(CASE WHEN address_side = 'source' AND ip_version = 4 THEN unique_address_count ELSE 0 END) AS saIpv4Count,
				SUM(CASE WHEN address_side = 'destination' AND ip_version = 4 THEN unique_address_count ELSE 0 END) AS daIpv4Count,
				SUM(CASE WHEN address_side = 'source' AND ip_version = 6 THEN unique_address_count ELSE 0 END) AS saIpv6Count,
				SUM(CASE WHEN address_side = 'destination' AND ip_version = 6 THEN unique_address_count ELSE 0 END) AS daIpv6Count
			FROM address_count_stats
			WHERE source_id = ?
				AND granularity = ?
				AND bucket_start = ?
				AND src_visibility = ?
				AND dst_visibility = ?
			GROUP BY source_id, bucket_start`,
			[router, FIVE_MINUTES, bucketStart, DEFAULT_SRC_VISIBILITY, DEFAULT_DST_VISIBILITY]
		);

		if (!row) {
			return json(
				{ error: `IP statistics not found for router ${router} at ${slug}` },
				{ status: 404 }
			);
		}

		const response = isSource
			? { ipv4Count: row.saIpv4Count, ipv6Count: row.saIpv6Count }
			: { ipv4Count: row.daIpv4Count, ipv6Count: row.daIpv6Count };

		return json(response);
	} catch (error) {
		console.error('Failed to fetch IP counts from database:', error);
		return json({ error: 'Failed to get IP counts' }, { status: 500 });
	}
};
