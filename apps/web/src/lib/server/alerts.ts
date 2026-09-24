import database from '#db';
import type { AlertHorizon, AlertSort, AlertsFeedResponse, AlertTail } from '#lib/types/types.ts';

const HORIZON_SECONDS: Record<AlertHorizon, number> = {
	'1h': 60 * 60,
	'6h': 6 * 60 * 60,
	'24h': 24 * 60 * 60,
	'7d': 7 * 24 * 60 * 60
};
const DEFAULT_HORIZON: AlertHorizon = '24h';

export type AlertsFeedOptions = {
	tail?: AlertTail;
	horizon?: AlertHorizon;
	sort?: AlertSort;
	limit?: number;
};

function absentFeed(horizonSeconds: number): AlertsFeedResponse {
	return {
		feed: { present: false },
		horizonSeconds,
		totalAddresses: 0,
		addresses: []
	};
}

export async function getAlertsFeedForDataset(
	datasetId: string,
	options: AlertsFeedOptions = {}
): Promise<AlertsFeedResponse> {
	const horizonSeconds = HORIZON_SECONDS[options.horizon ?? DEFAULT_HORIZON];
	const feed = await database.readAlertsFeed(datasetId, {
		horizonSeconds,
		tail: options.tail,
		sort: options.sort,
		limit: options.limit
	});
	return feed ?? absentFeed(horizonSeconds);
}
