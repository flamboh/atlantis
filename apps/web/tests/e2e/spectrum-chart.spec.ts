import { expect, test, type Locator, type Page } from '@playwright/test';

declare global {
	interface Window {
		spectrumText: string[];
	}
}

const DASHBOARD = '/datasets/playwright?startDate=2025-03-01&endDate=2025-03-01&groupBy=5min';

async function paintedPoint(canvas: Locator) {
	return canvas.evaluate((element: HTMLCanvasElement) => {
		const context = element.getContext('2d');
		if (!context) return null;
		const { data } = context.getImageData(0, 0, element.width, element.height);
		const points: number[] = [];
		for (let index = 0; index < data.length; index += 4) {
			const [r, g, b, a] = data.subarray(index, index + 4);
			if (a > 100 && Math.max(r, g, b) - Math.min(r, g, b) > 60) points.push(index / 4);
		}
		if (points.length === 0) return null;
		const pixel = points[Math.floor(points.length / 2)];
		const rect = element.getBoundingClientRect();
		return {
			x: rect.left + ((pixel % element.width) * rect.width) / element.width,
			y: rect.top + (Math.floor(pixel / element.width) * rect.height) / element.height
		};
	});
}

async function rendered(canvas: Locator) {
	await expect(canvas).toBeVisible();
	await expect.poll(() => paintedPoint(canvas)).not.toBeNull();
}

async function openSpectrum(page: Page) {
	await page.goto(DASHBOARD);
	const card = page.locator('[data-chart-id="spectrum"]');
	await card.scrollIntoViewIfNeeded();
	const canvas = card.getByLabel('Spectrum chart');
	await rendered(canvas);
	return { card, canvas };
}

async function hoverSpectrum(page: Page, canvas: Locator) {
	await canvas.scrollIntoViewIfNeeded();
	const point = await paintedPoint(canvas);
	expect(point).not.toBeNull();
	await page.mouse.move(point!.x, point!.y);
}

test('renders a singleton bucket and recovers from cached empty, unavailable and deselected states', async ({
	page
}) => {
	const errors: string[] = [];
	page.on('pageerror', (error) => errors.push(error.message));
	const { card, canvas } = await openSpectrum(page);
	const family = page.getByRole('group', { name: 'MAAD address family' });
	for (let attempt = 0; attempt < 2; attempt += 1) {
		await family.getByRole('button', { name: 'IPv6 (/23–/64)' }).click();
		await expect(card).toContainText('No source spectrum data');
		await family.getByRole('button', { name: 'IPv4 (/8–/24)' }).click();
		await rendered(canvas);
		await hoverSpectrum(page, canvas);
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
		await rendered(canvas);
	}
	await page.getByRole('checkbox', { name: 'fixture-router', exact: true }).uncheck();
	await expect(card).toContainText('Select at least one source');
	await page.getByRole('checkbox', { name: 'fixture-router', exact: true }).check();
	await rendered(canvas);
	await hoverSpectrum(page, canvas);
	await expect(card.getByRole('tooltip')).toBeVisible();
	expect(errors).toEqual([]);
});

test('keeps painted points and hover across granularity, direction, side, date and reset changes', async ({
	page
}) => {
	const { card, canvas } = await openSpectrum(page);
	for (const granularity of ['Day', 'Hour', '30 min', '10 min', '5 min']) {
		await page.getByRole('button', { name: granularity, exact: true }).click();
		await rendered(canvas);
		await hoverSpectrum(page, canvas);
		await expect(card.getByRole('tooltip')).toContainText('2025-03-01');
	}
	for (const direction of ['Ingress', 'Egress', 'Lateral', 'Transit', 'All']) {
		await page
			.getByRole('group', { name: 'Traffic direction' })
			.getByRole('button', { name: direction, exact: true })
			.click();
		await rendered(canvas);
	}
	await card.getByRole('button', { name: 'Destination', exact: true }).click();
	await rendered(canvas);
	await page.getByLabel('End Date', { exact: true }).fill('2025-03-02');
	await page.getByLabel('End Date', { exact: true }).press('Tab');
	await rendered(canvas);
	await page.getByLabel('Start Date', { exact: true }).fill('2025-03-02');
	await page.getByLabel('Start Date', { exact: true }).press('Tab');
	await expect(card).toContainText('No destination spectrum data');
	await page.getByLabel('Start Date', { exact: true }).fill('2025-03-01');
	await page.getByLabel('Start Date', { exact: true }).press('Tab');
	await rendered(canvas);
	await page.getByRole('button', { name: 'Reset View' }).click();
	await rendered(canvas);
	await hoverSpectrum(page, canvas);
	await expect(card.getByRole('tooltip')).toBeVisible();
});

test('aborts an older request when a newer filter is selected', async ({ page }) => {
	const { canvas } = await openSpectrum(page);
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
	await rendered(canvas);
	const before = await canvas.evaluate((element: HTMLCanvasElement) => element.toDataURL());
	release();
	await aborted;
	await page.unrouteAll({ behavior: 'wait' });
	await expect
		.poll(() => canvas.evaluate((element: HTMLCanvasElement) => element.toDataURL()))
		.toBe(before);
});

test('opens the time bucket under a spectrum point instead of using its flattened index', async ({
	page
}) => {
	const { canvas } = await openSpectrum(page);
	await hoverSpectrum(page, canvas);
	const point = await paintedPoint(canvas);
	await page.mouse.click(point!.x, point!.y);
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
	await expect(card.locator('canvas')).not.toBeAttached();
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
	await page.addInitScript(() => {
		const state = window;
		state.spectrumText = [];
		const fillText = CanvasRenderingContext2D.prototype.fillText;
		CanvasRenderingContext2D.prototype.fillText = function (...args) {
			state.spectrumText.push(args[0]);
			return fillText.apply(this, args);
		};
	});
	await page.goto('/netflow/files/202503010200?dataset=playwright');
	const canvas = page.getByLabel('Multifractal spectrum chart').first();
	await rendered(canvas);
	await page.getByRole('button', { name: 'Packets', exact: true }).click();
	await expect(canvas).not.toBeAttached();
	await page.getByRole('button', { name: 'Addresses', exact: true }).click();
	await rendered(canvas);
	await page.getByRole('button', { name: 'IPv6 (/23–/64)' }).click();
	await expect(canvas).not.toBeAttached();
	await page.getByRole('button', { name: 'IPv4 (/8–/24)' }).click();
	await rendered(canvas);
	await canvas.scrollIntoViewIfNeeded();
	await page.evaluate(() => {
		window.spectrumText = [];
	});
	await hoverSpectrum(page, canvas);
	await expect
		.poll(() =>
			page.evaluate(() => window.spectrumText.some((text) => text.startsWith('alpha = ')))
		)
		.toBe(true);
	const text = await page.evaluate(() => window.spectrumText);
	expect(text.some((value) => value.startsWith('f(alpha): '))).toBe(true);
	expect(text.some((value) => value.startsWith('y = x (reference):'))).toBe(false);
});
