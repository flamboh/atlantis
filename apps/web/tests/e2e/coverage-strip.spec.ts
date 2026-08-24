import { expect, test } from '@playwright/test';

test('coverage hover does not resize the chart card', async ({ page }) => {
	await page.goto('/datasets/playwright?startDate=2025-03-01&endDate=2025-03-01&groupBy=5min');

	const chartSection = page.locator('[data-chart-id="coverage"]');
	await chartSection.scrollIntoViewIfNeeded();
	await expect(chartSection).toHaveAttribute('data-chart-activated', 'true');

	const card = page.getByTestId('coverage-strip-card');
	const strip = page.getByTestId('coverage-strip');
	await expect(card).toBeVisible();
	await expect(strip).toBeVisible();
	await expect(strip.locator('canvas')).toBeVisible();

	const heightBeforeHover = await card.evaluate(
		(element) => element.getBoundingClientRect().height
	);
	await strip.locator('canvas').hover({ position: { x: 150, y: 10 } });
	const heightAfterHover = await card.evaluate((element) => element.getBoundingClientRect().height);

	expect(heightAfterHover).toBe(heightBeforeHover);
});
