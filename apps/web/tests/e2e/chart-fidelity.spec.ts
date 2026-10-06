import { expect, test } from '@playwright/test';
import { activateChart, expectRendered } from './chart-helpers';

test('time axes keep midnight and noon labels with bounded grid geometry', async ({ page }) => {
	await page.route('**/api/ip/stats?*', async (route) => {
		const response = await route.fetch();
		const payload = await response.json();
		for (const timeline of payload.timelines) {
			const bucket = timeline.buckets.find((bucket: { data: unknown }) => bucket.data);
			timeline.buckets = Array.from({ length: 288 }, (_, index) => ({
				...bucket,
				bucketStart: 1740816000 + index * 600,
				bucketEnd: 1740816000 + (index + 1) * 600
			}));
		}
		await route.fulfill({ response, json: payload });
	});
	await page.goto('/datasets/playwright?startDate=2025-03-01&endDate=2025-03-02&groupBy=10min');
	const card = await activateChart(page, 'ip');
	await expectRendered(card);
	const surface = card.getByTestId('chart-surface');
	for (const label of ['Sat 3/1 00:00', 'Sat 3/1 12:00', 'Sun 3/2 00:00', 'Sun 3/2 12:00']) {
		await expect(surface.locator('text').filter({ hasText: label })).toBeVisible();
	}
	await expect(surface.locator('path[data-ts-key^="temporal-grid-"]')).toHaveCount(2);
	const ticks = surface.locator('text').filter({ hasText: /3\/[12] (00|12):00/ });
	for (const tick of await ticks.all())
		await expect(tick).toHaveAttribute('transform', /rotate\(-45/);
});
