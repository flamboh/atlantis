import { expect, test, type Page } from '@playwright/test';

import { rendered, chartTarget, hoverChart } from './chart-helpers';

const DASHBOARD = '/datasets/playwright?startDate=2025-03-01&endDate=2025-03-01&groupBy=5min';

async function openSpectrum(page: Page) {
	await page.goto(DASHBOARD);
	const card = page.locator('[data-chart-id="spectrum"]');
	await card.scrollIntoViewIfNeeded();
	const surface = card.getByLabel('Spectrum chart');
	await rendered(surface);
	return { card, surface };
}

test('renders a singleton bucket and recovers from cached empty, unavailable and deselected states', async ({
	page
}) => {
	const errors: string[] = [];
	page.on('pageerror', (error) => errors.push(error.message));
	const { card, surface } = await openSpectrum(page);
	const family = page.getByRole('group', { name: 'MAAD address family' });
	for (let attempt = 0; attempt < 2; attempt += 1) {
		await family.getByRole('button', { name: 'IPv6 (/23–/64)' }).click();
		await expect(card).toContainText('No source spectrum data');
		await family.getByRole('button', { name: 'IPv4 (/8–/24)' }).click();
		await rendered(surface);
		await hoverChart(page, surface);
		await expect(card.getByRole('tooltip')).toContainText('2025-03-01 02:00');
	}
	for (const measure of ['Packets', 'Bytes']) {
		await page
			.getByRole('group', { name: 'MAAD measure' })
			.getByRole('button', { name: measure })
			.click();
		await expect(card.getByTestId('chart-unavailable')).toBeVisible();
		await expect(card.getByRole('tooltip')).not.toBeAttached();
		await page
			.getByRole('group', { name: 'MAAD measure' })
			.getByRole('button', { name: 'Addresses' })
			.click();
		await rendered(surface);
	}
	await page.getByRole('checkbox', { name: 'fixture-router', exact: true }).uncheck();
	await expect(card).toContainText('Select at least one source');
	await page.getByRole('checkbox', { name: 'fixture-router', exact: true }).check();
	await rendered(surface);
	await hoverChart(page, surface);
	await expect(card.getByRole('tooltip')).toBeVisible();
	expect(errors).toEqual([]);
});

test('keeps rendered points and hover across granularity, direction, side, date and reset changes', async ({
	page
}) => {
	const { card, surface } = await openSpectrum(page);
	for (const granularity of ['Day', 'Hour', '30 min', '10 min', '5 min']) {
		await page.getByRole('button', { name: granularity, exact: true }).click();
		await rendered(surface);
		await hoverChart(page, surface);
		await expect(card.getByRole('tooltip')).toContainText('2025-03-01');
	}
	for (const direction of ['Ingress', 'Egress', 'Lateral', 'Transit', 'All']) {
		await page
			.getByRole('group', { name: 'Traffic direction' })
			.getByRole('button', { name: direction, exact: true })
			.click();
		await rendered(surface);
	}
	await card.getByRole('button', { name: 'Destination', exact: true }).click();
	await rendered(surface);
	await page.getByLabel('End Date', { exact: true }).fill('2025-03-02');
	await page.getByLabel('End Date', { exact: true }).press('Tab');
	await rendered(surface);
	await page.getByLabel('Start Date', { exact: true }).fill('2025-03-02');
	await page.getByLabel('Start Date', { exact: true }).press('Tab');
	await expect(card).toContainText('No destination spectrum data');
	await page.getByLabel('Start Date', { exact: true }).fill('2025-03-01');
	await page.getByLabel('Start Date', { exact: true }).press('Tab');
	await rendered(surface);
	await page.getByRole('button', { name: 'Reset View' }).click();
	await rendered(surface);
	await hoverChart(page, surface);
	await expect(card.getByRole('tooltip')).toBeVisible();
});

test('aborts an older request when a newer filter is selected', async ({ page }) => {
	const { surface } = await openSpectrum(page);
	let release = () => {};
	const blocked = new Promise<void>((resolve) => {
		release = resolve;
	});
	await page.route('**/api/netflow/spectrum-stats?**', async (route) => {
		if (new URL(route.request().url()).searchParams.get('granularity') === '1h') await blocked;
		await route.continue();
	});
	const oldRequest = page.waitForRequest(
		(request) =>
			request.url().includes('/spectrum-stats?') &&
			new URL(request.url()).searchParams.get('granularity') === '1h'
	);
	await page.getByRole('button', { name: 'Hour', exact: true }).click();
	const pending = await oldRequest;
	const aborted = page.waitForEvent('requestfailed', {
		predicate: (request) => request === pending
	});
	await page.getByRole('button', { name: '10 min', exact: true }).click();
	await rendered(surface);
	const state = surface.locator('..').getByTestId('chart-render-state');
	const snapshot = () =>
		state.evaluate((element) => ({
			kind: element.getAttribute('data-kind'),
			marks: element.getAttribute('data-mark-count'),
			axes: Array.from(
				element.querySelectorAll('[data-testid="chart-axis"]'),
				(axis) => axis.textContent
			),
			series: Array.from(element.querySelectorAll('[data-testid="chart-series"]'), (series) => ({
				label: series.textContent,
				count: series.getAttribute('data-count'),
				min: series.getAttribute('data-min'),
				max: series.getAttribute('data-max'),
				total: series.getAttribute('data-total')
			}))
		}));
	const before = await snapshot();
	release();
	await aborted;
	await page.unrouteAll({ behavior: 'wait' });
	await expect.poll(snapshot).toEqual(before);
});

test('opens the time bucket under a spectrum point instead of using its flattened index', async ({
	page
}) => {
	const { surface } = await openSpectrum(page);
	await hoverChart(page, surface);
	const point = await chartTarget(surface);
	await page.mouse.click(point.x, point.y);
	await expect(page).toHaveURL(/\/netflow\/files\/202503010200\?/);
});

test('shows explicit empty states for invalid spectrum points on dashboard and file views', async ({
	page
}) => {
	await page.route('**/api/netflow/spectrum-stats?**', async (route) => {
		const response = await route.fetch();
		const body = await response.json();
		for (const timeline of body.timelines) {
			for (const bucket of timeline.buckets) {
				if (bucket.data) bucket.data.spectrumSa = [{ alpha: null, f: null }];
			}
		}
		await route.fulfill({ json: body });
	});
	await page.goto(DASHBOARD);
	const card = page.locator('[data-chart-id="spectrum"]');
	await card.scrollIntoViewIfNeeded();
	await expect(card).toContainText('No source spectrum data');
	await expect(card.getByTestId('chart-surface')).not.toBeAttached();
	await page.route('**/api/netflow/files/*/details?**', async (route) => {
		const response = await route.fetch();
		const body = await response.json();
		for (const router of body.routers) router.spectrumSource.spectrum = [{ alpha: null, f: null }];
		await route.fulfill({ json: body });
	});
	await page.goto('/netflow/files/202503010200?dataset=playwright');
	await expect(page.getByText('No finite spectrum data for this selection.')).toBeVisible();
});

test('file spectrum hover describes only a spectrum point after measure and family changes', async ({
	page
}) => {
	await page.goto('/netflow/files/202503010200?dataset=playwright');
	const surface = page.getByLabel('Multifractal spectrum chart').first();
	await rendered(surface);
	await page.getByRole('button', { name: 'Packets', exact: true }).click();
	await expect(surface).not.toBeAttached();
	await page.getByRole('button', { name: 'Addresses', exact: true }).click();
	await rendered(surface);
	await page.getByRole('button', { name: 'IPv6 (/23–/64)' }).click();
	await expect(surface).not.toBeAttached();
	await page.getByRole('button', { name: 'IPv4 (/8–/24)' }).click();
	await rendered(surface);
	await surface.scrollIntoViewIfNeeded();
	await hoverChart(page, surface);
	const tooltip = surface.locator('..').getByTestId('chart-tooltip');
	await expect(tooltip).toContainText('alpha = ');
	await expect(tooltip).toContainText('f(alpha): ');
	await expect(tooltip).not.toContainText('y = x (reference):');
});
