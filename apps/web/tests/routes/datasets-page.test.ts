import { describe, expect, it, vi } from 'vitest';
import { load } from '../../src/routes/datasets/[dataset]/+page';

function jsonResponse(payload: unknown, status = 200) {
	return { ok: status >= 200 && status < 300, status, json: async () => payload };
}

function mockDatasetFetch(maadStatus: ReturnType<typeof jsonResponse>) {
	return vi.fn(async (input: string) => {
		if (input === '/api/datasets') {
			return jsonResponse({
				data: [
					{
						datasetId: 'uoregon',
						label: 'UONet-in',
						defaultStartDate: '2025-02-11',
						discoveryMode: 'live',
						hasLocality: false,
						isDefault: true
					}
				],
				error: null
			});
		}
		if (input === '/api/routers?dataset=uoregon') {
			return jsonResponse(['router-a', 'router-b']);
		}
		if (input === '/api/netflow/maad-status?dataset=uoregon') {
			return maadStatus;
		}
		throw new Error(`Unexpected fetch ${input}`);
	});
}

describe('/datasets/[dataset] load', () => {
	it('returns the dataset page props from dataset metadata', async () => {
		const fetch = mockDatasetFetch(jsonResponse({ computed: true, internalSide: false }));

		const result = await load({
			params: { dataset: 'uoregon' },
			fetch
		} as never);

		expect(fetch).toHaveBeenCalledWith('/api/datasets');
		expect(fetch).toHaveBeenCalledWith('/api/routers?dataset=uoregon');
		expect(fetch).toHaveBeenCalledWith('/api/netflow/maad-status?dataset=uoregon');
		expect(result).toEqual({
			datasets: [
				{
					datasetId: 'uoregon',
					label: 'UONet-in',
					defaultStartDate: '2025-02-11',
					discoveryMode: 'live',
					hasLocality: false,
					isDefault: true
				}
			],
			datasetId: 'uoregon',
			title: 'UONet-in',
			defaultStartDate: '2025-02-11',
			hasLocality: false,
			maadComputed: true,
			maadInternalSide: false,
			routers: ['router-a', 'router-b']
		});
	});

	it('reports a dataset built without MAAD', async () => {
		const result = await load({
			params: { dataset: 'uoregon' },
			fetch: mockDatasetFetch(jsonResponse({ computed: false, internalSide: true }))
		} as never);

		expect(result).toMatchObject({ maadComputed: false, maadInternalSide: true });
	});

	it('fails when the MAAD status cannot be read', async () => {
		await expect(
			load({
				params: { dataset: 'uoregon' },
				fetch: mockDatasetFetch(jsonResponse({ error: 'Database query failed' }, 500))
			} as never)
		).rejects.toMatchObject({ status: 500, body: { message: 'Database query failed' } });

		await expect(
			load({
				params: { dataset: 'uoregon' },
				fetch: mockDatasetFetch(jsonResponse({ computed: 'yes', internalSide: true }))
			} as never)
		).rejects.toMatchObject({ status: 500, body: { message: 'Invalid MAAD status response' } });

		await expect(
			load({
				params: { dataset: 'uoregon' },
				fetch: mockDatasetFetch(jsonResponse({ computed: true }))
			} as never)
		).rejects.toMatchObject({ status: 500, body: { message: 'Invalid MAAD status response' } });
	});
});
