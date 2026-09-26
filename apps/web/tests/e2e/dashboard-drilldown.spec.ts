import { expect, test, type Page } from '@playwright/test';

const WIDE_DAILY_RANGE =
	'/datasets/playwright?startDate=2025-01-01&endDate=2025-03-31&groupBy=date';

async function clickChartCenter(page: Page, chartId: string) {
	const canvas = page.locator(`[data-chart-id="${chartId}"] canvas`).first();
	await expect(canvas).toBeVisible();
	await canvas.scrollIntoViewIfNeeded();
	const box = await canvas.boundingBox();
	if (!box) throw new Error(`Missing ${chartId} canvas bounds`);
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
