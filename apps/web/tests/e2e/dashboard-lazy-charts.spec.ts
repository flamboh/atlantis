import { expect, test } from '@playwright/test';
import { setDateRange } from './toolbar-helpers';

test('mounts chart cards near the viewport and keeps them mounted', async ({ page }) => {
	await page.goto('/datasets/playwright?startDate=2025-03-01&endDate=2025-03-01&groupBy=5min');

	const dashboard = page.locator('[data-chart-id="dashboard"]');
	const ip = page.locator('[data-chart-id="ip"]');
	await expect(dashboard).toHaveAttribute('data-chart-activated', 'true');
	await expect(ip).toHaveAttribute('data-chart-activated', 'false');
	await expect(page.getByTestId('deferred-chart-ip')).toBeAttached();

	await page.locator('[data-chart-sentinel="ip"]').scrollIntoViewIfNeeded();
	await expect(ip).toHaveAttribute('data-chart-activated', 'true');
	await expect(page.getByTestId('deferred-chart-ip')).not.toBeAttached();

	await dashboard.scrollIntoViewIfNeeded();
	await expect(ip).toHaveAttribute('data-chart-activated', 'true');
});

test('mounts only the persisted first card on initial load', async ({ page }) => {
	await page.addInitScript(() => {
		localStorage.setItem(
			'netflow-main-chart-order-v6',
			'["protocol","dashboard","characteristics","ports","ip","dimensions","spectrum","coverage"]'
		);
	});
	const requestedPaths: string[] = [];
	page.on('request', (request) => requestedPaths.push(new URL(request.url()).pathname));

	await page.goto('/datasets/playwright?startDate=2025-03-01&endDate=2025-03-01&groupBy=5min');

	await expect(page.locator('[data-chart-id="protocol"]')).toHaveAttribute(
		'data-chart-activated',
		'true'
	);
	await expect(page.locator('[data-chart-id="dashboard"]')).toHaveAttribute(
		'data-chart-activated',
		'false'
	);
	await expect.poll(() => requestedPaths.includes('/api/protocol/stats')).toBe(true);
	// The KPI row reads the traffic window up front; the deferred traffic card reuses it.
	await expect
		.poll(() => requestedPaths.filter((path) => path === '/api/netflow/stats').length)
		.toBe(1);
	for (const path of [
		'/api/ip/stats',
		'/api/netflow/characteristics',
		'/api/netflow/spectrum-stats'
	])
		expect(requestedPaths).not.toContain(path);
	await page.getByRole('button', { name: 'Load Traffic Overview chart' }).click();
	await expect(page.locator('[data-chart-id="dashboard"]')).toHaveAttribute(
		'data-chart-activated',
		'true'
	);
	await expect(page.getByTestId('chart-card-state').first()).toBeAttached();
	expect(requestedPaths.filter((path) => path === '/api/netflow/stats')).toHaveLength(1);
});

test('uses the latest filters when a deferred card mounts', async ({ page }) => {
	await page.goto('/datasets/playwright?startDate=2025-03-01&endDate=2025-03-01&groupBy=5min');
	await expect(page.locator('[data-chart-id="dashboard"]')).toHaveAttribute(
		'data-chart-activated',
		'true'
	);
	const ip = page.locator('[data-chart-id="ip"]');
	await expect(ip).toHaveAttribute('data-chart-activated', 'false');

	await setDateRange(page, { endDate: '2025-03-02' });
	await expect.poll(() => new URL(page.url()).searchParams.get('endDate')).toBe('2025-03-02');

	const requestPromise = page.waitForRequest(
		(request) => new URL(request.url()).pathname === '/api/ip/stats'
	);
	await page.getByRole('button', { name: 'Load Unique IP Counts chart' }).click();
	const request = await requestPromise;

	expect(new URL(request.url()).searchParams.get('endDate')).toBe('1740988800');
});

test('placeholder drag handles preserve custom chart ordering', async ({ page }) => {
	await page.goto('/datasets/playwright?startDate=2025-03-01&endDate=2025-03-01&groupBy=5min');

	await expect(page.locator('[data-chart-id="dashboard"]')).toHaveAttribute(
		'data-chart-activated',
		'true'
	);
	await expect(page.locator('[data-chart-id="protocol"]')).toHaveAttribute(
		'data-chart-activated',
		'false'
	);
	const source = page.locator('[data-chart-id="protocol"] [data-drag-handle]');
	const target = page.locator('[data-chart-id="characteristics"]');
	const dataTransfer = await page.evaluateHandle(() => new DataTransfer());
	await source.dispatchEvent('dragstart', { dataTransfer });
	await target.dispatchEvent('dragover', { dataTransfer });
	await target.dispatchEvent('drop', { dataTransfer });

	await expect
		.poll(() =>
			page
				.locator('[data-chart-id]')
				.evaluateAll((cards) => cards.map((card) => card.getAttribute('data-chart-id')))
		)
		.toEqual([
			'dashboard',
			'protocol',
			'characteristics',
			'ports',
			'ip',
			'dimensions',
			'spectrum',
			'coverage'
		]);
	await expect
		.poll(() => page.evaluate(() => localStorage.getItem('netflow-main-chart-order-v6')))
		.toBe(
			'["dashboard","protocol","characteristics","ports","ip","dimensions","spectrum","coverage"]'
		);
});
