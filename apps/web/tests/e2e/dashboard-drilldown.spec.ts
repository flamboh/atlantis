import { expect, test, type Page } from '@playwright/test';

const WIDE_DAILY_RANGE =
	'/datasets/playwright?startDate=2025-01-01&endDate=2025-03-31&groupBy=date';

async function clickChartCenter(page: Page, chartId: string) {
	const surface = page
		.locator(`[data-chart-id="${chartId}"] [data-testid="chart-surface"]`)
		.first();
	await expect(surface).toBeVisible();
	await surface.scrollIntoViewIfNeeded();
	const box = await surface.boundingBox();
	if (!box) throw new Error(`Missing ${chartId} surface bounds`);
	await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
}

function searchParam(page: Page, key: string) {
	return new URL(page.url()).searchParams.get(key);
}

test('traffic overview drill-down from a wide daily range switches to hourly', async ({ page }) => {
	const initialRequest = page.waitForRequest(
		(request) => new URL(request.url()).pathname === '/api/netflow/stats'
	);
	await page.goto(WIDE_DAILY_RANGE);
	expect(new URL((await initialRequest).url()).searchParams.get('groupBy')).toBe('date');
	await page.waitForLoadState('networkidle');

	const drillDownRequest = page.waitForRequest((request) => {
		const url = new URL(request.url());
		return url.pathname === '/api/netflow/stats' && url.searchParams.get('groupBy') === 'hour';
	});
	await clickChartCenter(page, 'dashboard');

	await expect.poll(() => searchParam(page, 'groupBy')).toBe('hour');
	expect(searchParam(page, 'startDate')).not.toBe('2025-01-01');
	expect(searchParam(page, 'endDate')).not.toBe('2025-03-31');
	await drillDownRequest;
});

test('IP breakdown drill-down from a wide daily range switches to hourly', async ({ page }) => {
	await page.goto(WIDE_DAILY_RANGE);
	const initialRequest = page.waitForRequest(
		(request) => new URL(request.url()).pathname === '/api/ip/stats'
	);
	await page.locator('[data-chart-sentinel="ip"]').scrollIntoViewIfNeeded();
	expect(new URL((await initialRequest).url()).searchParams.get('granularity')).toBe('1d');
	await page.waitForLoadState('networkidle');

	const drillDownRequest = page.waitForRequest((request) => {
		const url = new URL(request.url());
		return url.pathname === '/api/ip/stats' && url.searchParams.get('granularity') === '1h';
	});
	await clickChartCenter(page, 'ip');

	await expect.poll(() => searchParam(page, 'groupBy')).toBe('hour');
	expect(searchParam(page, 'startDate')).not.toBe('2025-01-01');
	expect(searchParam(page, 'endDate')).not.toBe('2025-03-31');
	await drillDownRequest;
});

test('MAAD dimensions drill-down keeps the MAAD measure while switching to hourly', async ({
	page
}) => {
	await page.goto(`${WIDE_DAILY_RANGE}&measure=packets`);
	const initialRequest = page.waitForRequest(
		(request) => new URL(request.url()).pathname === '/api/netflow/dimension-stats'
	);
	await page.locator('[data-chart-sentinel="dimensions"]').scrollIntoViewIfNeeded();
	const initialUrl = new URL((await initialRequest).url());
	expect(initialUrl.searchParams.get('granularity')).toBe('1d');
	expect(initialUrl.searchParams.get('measure')).toBe('packets');
	await page.waitForLoadState('networkidle');

	const drillDownRequest = page.waitForRequest((request) => {
		const url = new URL(request.url());
		return (
			url.pathname === '/api/netflow/dimension-stats' &&
			url.searchParams.get('granularity') === '1h' &&
			url.searchParams.get('measure') === 'packets'
		);
	});
	await clickChartCenter(page, 'dimensions');

	await expect.poll(() => searchParam(page, 'groupBy')).toBe('hour');
	expect(searchParam(page, 'measure')).toBe('packets');
	expect(searchParam(page, 'startDate')).not.toBe('2025-01-01');
	expect(searchParam(page, 'endDate')).not.toBe('2025-03-31');
	await drillDownRequest;
});
