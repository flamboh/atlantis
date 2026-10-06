import { expect, test, type Page } from '@playwright/test';

const DASHBOARD = '/datasets/playwright?startDate=2025-03-01&endDate=2025-03-01&groupBy=5min';

function measureControl(page: Page) {
	return page.getByRole('group', { name: 'MAAD measure' });
}

async function activateCard(page: Page, chartId: string) {
	await page.locator(`[data-chart-sentinel="${chartId}"]`).scrollIntoViewIfNeeded();
	await expect(page.locator(`[data-chart-id="${chartId}"]`)).toHaveAttribute(
		'data-chart-activated',
		'true'
	);
}

function waitForDimensions(page: Page, measure: string, ipVersion = '4') {
	return page.waitForRequest((request) => {
		const url = new URL(request.url());
		return (
			url.pathname === '/api/netflow/dimension-stats' &&
			url.searchParams.get('measure') === measure &&
			url.searchParams.get('ipVersion') === ipVersion
		);
	});
}

test('switches dashboard MAAD views between addresses, packets and bytes', async ({ page }) => {
	await page.goto(DASHBOARD);
	await expect(measureControl(page).getByRole('button', { name: 'Addresses' })).toHaveAttribute(
		'aria-pressed',
		'true'
	);

	const addressesRequest = waitForDimensions(page, 'addresses');
	await activateCard(page, 'dimensions');
	await addressesRequest;
	const dimensions = page.locator('[data-chart-id="dimensions"]');
	await expect(dimensions.getByLabel('MAAD dimensions chart')).toBeVisible();

	await activateCard(page, 'spectrum');
	const spectrum = page.locator('[data-chart-id="spectrum"]');
	await expect(spectrum.getByLabel('Spectrum chart')).toBeVisible();

	for (const [label, measure] of [
		['Packets', 'packets'],
		['Bytes', 'bytes']
	] as const) {
		const request = waitForDimensions(page, measure);
		await measureControl(page).getByRole('button', { name: label }).click();
		await request;
		await expect.poll(() => new URL(page.url()).searchParams.get('measure')).toBe(measure);
		await expect(dimensions.getByLabel('MAAD dimensions chart')).toBeVisible();
		await expect(spectrum.getByTestId('chart-unavailable')).toContainText(
			'only computed for the Addresses measure'
		);
		await expect(spectrum.getByRole('group', { name: 'Spectrum source' })).not.toBeAttached();
	}

	await measureControl(page).getByRole('button', { name: 'Addresses' }).click();
	await expect.poll(() => new URL(page.url()).searchParams.has('measure')).toBe(false);
	await expect(spectrum.getByTestId('chart-unavailable')).not.toBeAttached();
	await expect(spectrum.getByLabel('Spectrum chart')).toBeVisible();
});

test('shares one MAAD address family control across both MAAD cards', async ({ page }) => {
	await page.goto(DASHBOARD);
	await activateCard(page, 'dimensions');
	await activateCard(page, 'spectrum');
	const familyControl = page.getByRole('group', { name: 'MAAD address family' });
	await expect(familyControl).toHaveCount(1);

	const dimensionsRequest = waitForDimensions(page, 'addresses', '6');
	const spectrumRequest = page.waitForRequest((request) => {
		const url = new URL(request.url());
		return (
			url.pathname === '/api/netflow/spectrum-stats' && url.searchParams.get('ipVersion') === '6'
		);
	});
	await familyControl.getByRole('button', { name: 'IPv6 (/23–/64)' }).click();
	await dimensionsRequest;
	await spectrumRequest;
});

test('charts one MAAD dimension and address side at a time', async ({ page }) => {
	await page.goto(DASHBOARD);
	await activateCard(page, 'dimensions');
	const dimensions = page.locator('[data-chart-id="dimensions"]');
	const side = dimensions.getByRole('group', { name: 'MAAD address side' });
	const order = dimensions.getByRole('group', { name: 'MAAD dimension' });
	await expect(side.getByRole('button', { name: 'Source' })).toHaveAttribute(
		'aria-pressed',
		'true'
	);
	await expect(order.getByRole('button', { name: 'D1' })).toHaveAttribute('aria-pressed', 'true');

	await side.getByRole('button', { name: 'Destination' }).click();
	await order.getByRole('button', { name: 'D2' }).click();
	await expect(side.getByRole('button', { name: 'Destination' })).toHaveAttribute(
		'aria-pressed',
		'true'
	);
	await expect(order.getByRole('button', { name: 'D2' })).toHaveAttribute('aria-pressed', 'true');
	await expect(order.getByRole('button', { name: 'D1' })).toHaveAttribute('aria-pressed', 'false');
	await expect(dimensions.getByLabel('MAAD dimensions chart')).toBeVisible();
});

test('rejects an unknown measure param in favor of addresses', async ({ page }) => {
	const requestedMeasures: string[] = [];
	page.on('request', (request) => {
		const url = new URL(request.url());
		if (url.pathname === '/api/netflow/dimension-stats') {
			requestedMeasures.push(url.searchParams.get('measure') ?? '');
		}
	});

	await page.goto(`${DASHBOARD}&measure=flows`);
	await expect(measureControl(page).getByRole('button', { name: 'Addresses' })).toHaveAttribute(
		'aria-pressed',
		'true'
	);
	await activateCard(page, 'dimensions');
	await expect.poll(() => requestedMeasures).toEqual(['addresses']);
});

test('keeps the weighted measure when drilling down to a file', async ({ page }) => {
	await page.goto(`${DASHBOARD}&measure=bytes`);
	await activateCard(page, 'dimensions');
	const surface = page.locator('[data-chart-id="dimensions"]').getByLabel('MAAD dimensions chart');
	await expect(surface).toBeVisible();

	const detailsRequest = page.waitForRequest(
		(request) => new URL(request.url()).pathname === '/api/netflow/files/202503010200/details'
	);
	await surface.click();

	await page.waitForURL(/\/netflow\/files\/202503010200\?/);
	const url = new URL(page.url());
	expect(url.searchParams.get('dataset')).toBe('playwright');
	expect(url.searchParams.get('measure')).toBe('bytes');
	expect(new URL((await detailsRequest).url()).searchParams.get('measure')).toBe('bytes');
	await expect(measureControl(page).getByRole('button', { name: 'Bytes' })).toHaveAttribute(
		'aria-pressed',
		'true'
	);

	const packetsRequest = page.waitForRequest((request) => {
		const requestUrl = new URL(request.url());
		return (
			requestUrl.pathname === '/api/netflow/files/202503010200/details' &&
			requestUrl.searchParams.get('measure') === 'packets'
		);
	});
	await measureControl(page).getByRole('button', { name: 'Packets' }).click();
	await expect.poll(() => new URL(page.url()).searchParams.get('measure')).toBe('packets');
	await packetsRequest;
});
