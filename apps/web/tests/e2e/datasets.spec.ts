import { expect, test } from '@playwright/test';

test('dataset metadata is served from the fixture product', async ({ request }) => {
	const response = await request.get('/api/datasets');

	expect(response.ok()).toBe(true);
	await expect(response.json()).resolves.toEqual({
		data: [
			{
				datasetId: 'playwright',
				label: 'Playwright Fixture',
				defaultStartDate: '2025-03-01',
				discoveryMode: 'static',
				hasLocality: true,
				isDefault: true
			}
		],
		error: null
	});
});
