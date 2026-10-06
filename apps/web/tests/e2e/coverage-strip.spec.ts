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
	await expect(strip.getByTestId('chart-surface')).toBeVisible();

	const heightBeforeHover = await card.evaluate(
		(element) => element.getBoundingClientRect().height
	);
	await strip.getByTestId('chart-surface').hover({ position: { x: 150, y: 10 } });
	const heightAfterHover = await card.evaluate((element) => element.getBoundingClientRect().height);

	expect(heightAfterHover).toBe(heightBeforeHover);
});

test('wide coverage intervals retain hover throughout their interiors', async ({ page }) => {
	await page.goto('/datasets/playwright?startDate=2025-03-01&endDate=2025-03-01');
	const card = page.locator('[data-chart-id="coverage"]');
	await card.scrollIntoViewIfNeeded();
	const surface = card.getByTestId('chart-surface');
	await expect(surface).toHaveAttribute('data-chart-rendered', 'true');
	const target = card.locator('[data-testid="chart-series"][data-point-x]').first();
	const x = Number(await target.getAttribute('data-point-x'));
	const y = Number(await target.getAttribute('data-point-y'));
	const box = await surface.boundingBox();
	if (!box) throw new Error('Missing coverage surface');
	for (const offset of [-100, 0, 100]) {
		await surface.hover({ position: { x: x * box.width + offset, y: y * box.height } });
		await expect(card.getByTestId('chart-tooltip')).toContainText('2025-03-01');
		await expect(card.getByTestId('chart-tooltip')).toContainText('fixture-router');
	}
});
