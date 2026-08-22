import { describe, expect, it, vi } from 'vitest';
import { GET } from '../../src/routes/api/datasets/+server';
import { listDatasetSummaries } from '$lib/server/datasets';

vi.mock('$lib/server/datasets', () => ({
	listDatasetSummaries: vi.fn()
}));

describe('/api/datasets GET', () => {
	it('returns dataset summaries in a stable envelope', async () => {
		vi.mocked(listDatasetSummaries).mockResolvedValue([
			{
				datasetId: 'uoregon',
				label: 'UONet-in',
				defaultStartDate: '2025-02-11',
				discoveryMode: 'live',
				isDefault: true
			}
		]);

		const response = await GET({} as never);

		expect(response.status).toBe(200);
		await expect(response.json()).resolves.toEqual({
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
		});
	});

	it('returns a 500 envelope when dataset summary loading fails', async () => {
		vi.mocked(listDatasetSummaries).mockImplementation(async () => {
			throw new Error('boom');
		});

		const response = await GET({} as never);

		expect(response.status).toBe(500);
		await expect(response.json()).resolves.toEqual({
			data: null,
			error: 'Failed to list datasets'
		});
	});
});
