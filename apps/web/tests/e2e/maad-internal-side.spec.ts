import { expect, test, type Page } from '@playwright/test';
import { chooseSegment, setDirection } from './toolbar-helpers';

const DASHBOARD =
	'/datasets/playwright-external-maad?startDate=2025-03-01&endDate=2025-03-01&groupBy=5min';

async function activateCard(page: Page, chartId: string) {
	await page.locator(`[data-chart-sentinel="${chartId}"]`).scrollIntoViewIfNeeded();
	await expect(page.locator(`[data-chart-id="${chartId}"]`)).toHaveAttribute(
		'data-chart-activated',
		'true'
	);
}

test('explains MAAD cards for internal address sides the product did not compute', async ({
	page
}) => {
	await page.goto(`${DASHBOARD}&direction=ingress`);
	await activateCard(page, 'dimensions');
	await activateCard(page, 'spectrum');
	const dimensions = page.locator('[data-chart-id="dimensions"]');
	const spectrum = page.locator('[data-chart-id="spectrum"]');

	await expect(dimensions.getByLabel('MAAD dimensions chart')).toBeVisible();
	await expect(spectrum.getByLabel('Spectrum chart')).toBeVisible();

	await chooseSegment(dimensions, 'MAAD address side', 'Destination');
	await expect(dimensions.getByTestId('chart-selection-unavailable')).toHaveText(
		'MAAD is not computed for internal addresses. The destination addresses are internal for ingress traffic.'
	);

	await chooseSegment(spectrum, 'Spectrum address side', 'Destination');
	await expect(spectrum.getByTestId('chart-selection-unavailable')).toContainText(
		'not computed for internal addresses'
	);

	await chooseSegment(spectrum, 'Spectrum address side', 'Source');
	await expect(spectrum.getByLabel('Spectrum chart')).toBeVisible();

	await setDirection(page, 'Lateral');
	await expect(dimensions.getByTestId('chart-unavailable')).toHaveText(
		'MAAD is not computed for internal addresses. The source and destination addresses are both internal for lateral traffic.'
	);
	await expect(spectrum.getByTestId('chart-unavailable')).toContainText(
		'not computed for internal addresses'
	);

	await setDirection(page, 'Transit');
	await expect(dimensions.getByLabel('MAAD dimensions chart')).toBeVisible();
	await expect(dimensions.getByTestId('chart-selection-unavailable')).not.toBeAttached();
});

test('keeps every side available for a product that computed internal-side MAAD', async ({
	page
}) => {
	await page.goto(
		'/datasets/playwright?startDate=2025-03-01&endDate=2025-03-01&groupBy=5min&direction=lateral'
	);
	await activateCard(page, 'dimensions');
	const dimensions = page.locator('[data-chart-id="dimensions"]');
	await expect(dimensions.getByLabel('MAAD dimensions chart')).toBeVisible();
	await expect(dimensions.getByTestId('chart-unavailable')).not.toBeAttached();
});

test('explains the file view side the product did not compute', async ({ page }) => {
	await page.route('**/api/netflow/files/**', async (route) => {
		const url = new URL(route.request().url());
		url.searchParams.set('direction', 'all');
		await route.fulfill({ response: await route.fetch({ url: url.toString() }) });
	});
	await page.goto('/netflow/files/202503010200?dataset=playwright-external-maad&direction=egress');

	const unavailable = page.getByTestId('maad-side-unavailable');
	await expect(unavailable.first()).toHaveText(
		'MAAD is not computed for internal addresses. The source addresses are internal for egress traffic.'
	);
	await expect(unavailable).toHaveCount(2);
	await expect(page.getByTestId('chart-surface').first()).toBeVisible();
});
