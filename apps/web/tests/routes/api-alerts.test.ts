import { beforeEach, describe, expect, it, vi } from 'vitest';
import { getAlertsFeedForDataset } from '$lib/server/alerts';
import { getRequestedDataset } from '$lib/server/datasets';
import { GET } from '../../src/routes/api/alerts/+server';

vi.mock('$lib/server/alerts', () => ({
	getAlertsFeedForDataset: vi.fn()
}));

vi.mock('$lib/server/datasets', () => ({
	getRequestedDataset: vi.fn()
}));

const PRESENT_RESPONSE = {
	feed: {
		present: true as const,
		latestWindowStart: 1_700_000_300,
		latestProcessedAt: 1_700_000_620,
		thresholds: { high: 3.5, low: 0.4 }
	},
	windows: [
		{
			windowStart: 1_700_000_300,
			windowEnd: 1_700_000_600,
			addressCount: 48_001,
			alertCount: 1,
			alerts: [{ address: '1.1.1.1', alpha: 3.9, tail: 'high' as const, rank: 1, r2: 0.98 }]
		}
	]
};

function eventFor(query = '') {
	return {
		url: new URL(`http://localhost/api/alerts${query}`),
		platform: undefined
	} as never;
}

describe('/api/alerts GET', () => {
	beforeEach(() => {
		vi.mocked(getRequestedDataset).mockReset().mockResolvedValue('alpha');
		vi.mocked(getAlertsFeedForDataset).mockReset().mockResolvedValue(PRESENT_RESPONSE);
	});

	it('returns the exact present-feed response and default query options', async () => {
		const response = await GET(eventFor('?dataset=alpha'));

		expect(response.status).toBe(200);
		await expect(response.json()).resolves.toEqual(PRESENT_RESPONSE);
		expect(getRequestedDataset).toHaveBeenCalledWith(
			new URL('http://localhost/api/alerts?dataset=alpha'),
			undefined
		);
		expect(getAlertsFeedForDataset).toHaveBeenCalledWith('alpha', {
			platform: undefined,
			tail: undefined,
			limitWindows: 24,
			before: undefined
		});
	});

	it('passes a valid tail filter through and rejects an invalid tail', async () => {
		const validResponse = await GET(eventFor('?tail=low'));
		expect(validResponse.status).toBe(200);
		expect(getAlertsFeedForDataset).toHaveBeenLastCalledWith(
			'alpha',
			expect.objectContaining({ tail: 'low' })
		);

		const invalidResponse = await GET(eventFor('?tail=middle'));
		expect(invalidResponse.status).toBe(400);
		await expect(invalidResponse.json()).resolves.toEqual({
			data: null,
			error: 'Invalid tail parameter'
		});
	});

	it.each([
		['?limitWindows=0', 1],
		['?limitWindows=999', 288]
	])('clamps %s before passing the window limit to the data layer', async (query, expected) => {
		const response = await GET(eventFor(query));

		expect(response.status).toBe(200);
		expect(getAlertsFeedForDataset).toHaveBeenCalledWith(
			'alpha',
			expect.objectContaining({ limitWindows: expected })
		);
	});

	it('rejects a non-numeric window limit', async () => {
		const response = await GET(eventFor('?limitWindows=many'));

		expect(response.status).toBe(400);
		await expect(response.json()).resolves.toEqual({
			data: null,
			error: 'Invalid limitWindows parameter'
		});
	});

	it('parses an exclusive before cursor and rejects an invalid cursor', async () => {
		const validResponse = await GET(eventFor('?before=1700000300'));
		expect(validResponse.status).toBe(200);
		expect(getAlertsFeedForDataset).toHaveBeenLastCalledWith(
			'alpha',
			expect.objectContaining({ before: 1_700_000_300 })
		);

		const invalidResponse = await GET(eventFor('?before=1700000300.5'));
		expect(invalidResponse.status).toBe(400);
		await expect(invalidResponse.json()).resolves.toEqual({
			data: null,
			error: 'Invalid before parameter'
		});
	});

	it('returns an absent feed as a normal 200 response', async () => {
		const absentResponse = { feed: { present: false as const }, windows: [] };
		vi.mocked(getAlertsFeedForDataset).mockResolvedValue(absentResponse);

		const response = await GET(eventFor());

		expect(response.status).toBe(200);
		await expect(response.json()).resolves.toEqual(absentResponse);
	});
});
