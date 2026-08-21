import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { getAlertsFeedForDataset } from '$lib/server/alerts';
import { getRequestedDataset } from '$lib/server/datasets';
import type { AlertsFeedResponse, AlertTail } from '$lib/types/types';

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

	const parsedLimitWindows = parseInteger(url.searchParams.get('limitWindows'));
	if (parsedLimitWindows === null) {
		return errorResponse('Invalid limitWindows parameter', 400);
	}
	const limitWindows = Math.min(288, Math.max(1, parsedLimitWindows ?? 24));

	const before = parseInteger(url.searchParams.get('before'));
	if (before === null) {
		return errorResponse('Invalid before parameter', 400);
	}

	try {
		const dataset = await getRequestedDataset(url, platform);
		const response: AlertsFeedResponse = await getAlertsFeedForDataset(dataset, {
			platform,
			tail: tail ?? undefined,
			limitWindows,
			before
		});
		return json(response);
	} catch (error) {
		console.error('Failed to load alerts feed:', error);
		return errorResponse('Failed to load alerts feed', 500);
	}
};
