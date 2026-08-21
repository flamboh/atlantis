import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { getAlertsFeedForDataset } from '$lib/server/alerts';
import { getRequestedDataset } from '$lib/server/datasets';
import type { AlertHorizon, AlertsFeedResponse, AlertSort, AlertTail } from '$lib/types/types';

type ErrorResponse = {
	data: null;
	error: string;
};

function parseInteger(value: string | null): number | undefined | null {
	if (value === null) {
		return undefined;
	}
	const normalized = value.trim();
	if (!/^-?\d+$/.test(normalized)) {
		return null;
	}

	const parsed = Number(normalized);
	return Number.isSafeInteger(parsed) ? parsed : null;
}

function errorResponse(message: string, status: number): Response {
	const response: ErrorResponse = { data: null, error: message };
	return json(response, { status });
}

export const GET: RequestHandler = async ({ url, platform }) => {
	const tailParam = url.searchParams.get('tail');
	if (tailParam !== null && tailParam !== 'high' && tailParam !== 'low') {
		return errorResponse('Invalid tail parameter', 400);
	}
	const tail = tailParam as AlertTail | null;

	const horizonParam = url.searchParams.get('horizon') ?? '24h';
	if (
		horizonParam !== '1h' &&
		horizonParam !== '6h' &&
		horizonParam !== '24h' &&
		horizonParam !== '7d'
	) {
		return errorResponse('Invalid horizon parameter', 400);
	}
	const horizon = horizonParam as AlertHorizon;

	const sortParam = url.searchParams.get('sort') ?? 'extreme';
	if (sortParam !== 'extreme' && sortParam !== 'recent') {
		return errorResponse('Invalid sort parameter', 400);
	}
	const sort = sortParam as AlertSort;

	const parsedLimit = parseInteger(url.searchParams.get('limit'));
	if (parsedLimit === null) {
		return errorResponse('Invalid limit parameter', 400);
	}
	const limit = Math.min(500, Math.max(1, parsedLimit ?? 100));

	try {
		const dataset = await getRequestedDataset(url, platform);
		const response: AlertsFeedResponse = await getAlertsFeedForDataset(dataset, {
			platform,
			tail: tail ?? undefined,
			horizon,
			sort,
			limit
		});
		return json(response);
	} catch (error) {
		console.error('Failed to load alerts feed:', error);
		return errorResponse('Failed to load alerts feed', 500);
	}
};
