import { describe, expect, it, vi } from 'vitest';
import { load } from '../../src/routes/datasets/[dataset]/+page';

describe('/datasets/[dataset] load', () => {
	it('returns the dataset page props from dataset metadata', async () => {
		const fetch = vi
			.fn()
			.mockResolvedValueOnce({
				ok: true,
				status: 200,
				json: async () => ({
					data: [
						{
							datasetId: 'uoregon',
							label: 'UONet-in',
							defaultStartDate: '2025-02-11',
							discoveryMode: 'live',
							isDefault: true
						}
					],
					error: null
				})
			})
			.mockResolvedValueOnce({
				ok: true,
				status: 200,
				json: async () => ['router-a', 'router-b']
			});

		const result = await load({
			params: { dataset: 'uoregon' },
			fetch
		} as never);

		expect(fetch).toHaveBeenNthCalledWith(1, '/api/datasets');
		expect(fetch).toHaveBeenNthCalledWith(2, '/api/routers?dataset=uoregon');
		expect(result).toEqual({
			datasetId: 'uoregon',
			title: 'UONet-in',
			defaultStartDate: '2025-02-11',
			routers: ['router-a', 'router-b']
		});
	});
});
