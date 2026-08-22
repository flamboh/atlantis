import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import {
	IP_GRANULARITIES,
	type IpGranularity,
	type NetflowCoverageResponse
} from '$lib/types/types';
import { buildCoverageOnlyTimelines } from '$lib/server/db/coverage';
import { getDatasetDb, getRequestedDataset } from '$lib/server/datasets';
import {
	groupByToGranularity,
	parseIpGranularity,
	parseSourceIds,
	parseTimestamp
} from '$lib/server/netflow-v3';
import { epochToPSTComponents } from '$lib/utils/timezone';

const GRANULARITY_SECONDS: Record<Exclude<IpGranularity, '1d'>, number> = {
	'5m': 5 * 60,
	'30m': 30 * 60,
	'1h': 60 * 60
};

function parseGranularity(url: URL): IpGranularity | null {
	const explicit = url.searchParams.get('granularity');
	if (explicit !== null) {
		return parseIpGranularity(explicit);
	}

	return groupByToGranularity(url.searchParams.get('groupBy') || 'date');
}

export const GET: RequestHandler = async ({ url, platform }) => {
	const routers = [...new Set(parseSourceIds(url.searchParams.get('routers')))];
	const granularity = parseGranularity(url);
	const start = parseTimestamp(url.searchParams.get('startDate'));
	const end = parseTimestamp(url.searchParams.get('endDate'));

	if (routers.length === 0) {
		return json({ error: 'No routers selected' }, { status: 400 });
	}

	if (granularity === null) {
		return json(
			{ error: `Invalid granularity. Expected one of: ${IP_GRANULARITIES.join(', ')}` },
			{ status: 400 }
		);
	}

	if (start === null || end === null) {
		return json({ error: 'Invalid start or end time' }, { status: 400 });
	}

	if (start >= end) {
		return json({ error: 'Start time must be before end time' }, { status: 400 });
	}

	if (!isAlignedToGranularity(start, granularity) || !isAlignedToGranularity(end, granularity)) {
		return json(
			{ error: `Start and end times must align to ${granularity} bucket boundaries` },
			{ status: 400 }
		);
	}

	try {
		const dataset = await getRequestedDataset(url, platform);
		const db = await getDatasetDb(dataset, platform);
		const timelines = await buildCoverageOnlyTimelines({
			db,
			granularity,
			start,
			end,
			sourceIds: routers
		});

		return json({ timelines, requestedRouters: routers } satisfies NetflowCoverageResponse);
	} catch (error) {
		console.error('Failed to query bucket_coverage:', error);
		const message = error instanceof Error ? error.message : 'Database query failed';
		const status = message.startsWith('Unknown dataset') ? 400 : 500;
		return json({ error: status === 400 ? message : 'Database query failed' }, { status });
	}
};

function isAlignedToGranularity(timestamp: number, granularity: IpGranularity): boolean {
	if (!Number.isInteger(timestamp)) {
		return false;
	}

	if (granularity === '1d') {
		const components = epochToPSTComponents(timestamp);
		return components.hours === 0 && components.minutes === 0 && components.seconds === 0;
	}

	return timestamp % GRANULARITY_SECONDS[granularity] === 0;
}
