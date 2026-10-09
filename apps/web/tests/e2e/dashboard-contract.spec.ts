import { expect, test } from '@playwright/test';
import { openSources, setDateRange, setInterval, setSource, sourceOption } from './toolbar-helpers';
import { chartTarget, rendered } from './chart-helpers';

const DASHBOARD = '/datasets/playwright?startDate=2025-03-01&endDate=2025-03-01&groupBy=5min';

test('legend clicks hide and restore a rendered traffic series', async ({ page }) => {
	await page.goto(DASHBOARD);
	const surface = page.locator('[data-chart-id="dashboard"]').getByTestId('chart-surface');
	const state = await rendered(surface);
	const tcp = state.getByTestId('chart-series').filter({ hasText: /^Flows TCP$/ });
	const target = await chartTarget(surface, 'legend');
	await page.mouse.click(target.x, target.y);
	await expect(tcp).toHaveAttribute('data-visible', 'false');
	await expect(state).toHaveAttribute('data-series-count', '3');
	await page.mouse.click(target.x, target.y);
	await expect(tcp).toHaveAttribute('data-visible', 'true');
	await expect(state).toHaveAttribute('data-series-count', '4');
});

test('range updates change values, empty bounds recover, and source defaults recover on reload', async ({
	page
}) => {
	await page.goto(DASHBOARD);
	const card = page.locator('[data-chart-id="dashboard"]');
	await rendered(card.getByTestId('chart-surface'));
	await setInterval(page, 'Day');
	const tcp = card.getByTestId('chart-series').filter({ hasText: /^Flows TCP$/ });
	await expect(tcp).toHaveAttribute('data-total', '100');
	await setDateRange(page, { endDate: '2025-03-03' });
	await expect(tcp).toHaveAttribute('data-total', '300');
	await setDateRange(page, { startDate: '2025-03-03' });
	await expect(tcp).toHaveAttribute('data-total', '200');
	await setSource(page, 'fixture-router', false);
	await page.reload();
	const sources = await openSources(page);
	await expect(sourceOption(sources, 'fixture-router')).toHaveAttribute('aria-checked', 'true');
	await page.keyboard.press('Escape');
	await expect(tcp).toHaveAttribute('data-total', '200');
	await page.goto('/datasets/playwright?startDate=2020-01-01&endDate=2020-01-02');
	await expect(card.getByTestId('chart-render-state')).toHaveAttribute('data-state', 'empty');
	await expect(card.getByTestId('chart-render-state')).toHaveAttribute('data-mark-count', '0');
	await page.goto(DASHBOARD);
	await expect(card.getByTestId('chart-series').first()).toHaveAttribute('data-total', '100');
});
