import { expect, test } from '@playwright/test';
import { FIXTURE_DASHBOARD, expectRendered } from './chart-helpers';
import { setDateRange } from './toolbar-helpers';

test('reversed windows do not fetch a normalized traffic range', async ({ page }) => {
	const statsRequests: string[] = [];
	page.on('request', (request) => {
		if (new URL(request.url()).pathname === '/api/netflow/stats') statsRequests.push(request.url());
	});
	await page.goto('/datasets/playwright?startDate=2025-03-03&endDate=2025-03-01');
	await expect(page.getByRole('alert')).toHaveText('Start Date must be on or before End Date.');
	await expect(page.locator('[data-kpi="flows"]')).toContainText('Unavailable');
	expect(statsRequests).toEqual([]);
	await setDateRange(page, { startDate: '2025-03-01' });
	await expectRendered(page.locator('[data-chart-id="dashboard"]'));
	await expect(page.locator('[data-kpi="flows"] [data-kpi-value]')).toHaveAttribute(
		'data-kpi-value',
		/^[1-9]\d*$/
	);
});

test('cached windows never display totals from a different selected window', async ({ page }) => {
	await page.goto(FIXTURE_DASHBOARD);
	const value = page.locator('[data-kpi="flows"] [data-kpi-value]');
	await expect(value).toHaveAttribute('data-kpi-value', '162');
	await setDateRange(page, { startDate: '2025-03-02', endDate: '2025-03-02' });
	await expect(value).toHaveAttribute('data-kpi-value', '0');
	await expect(
		page.locator('[data-chart-id="dashboard"]').getByTestId('chart-render-state')
	).toHaveAttribute('data-state', 'empty');
	await page.evaluate(() => {
		const failures: string[] = [];
		Object.assign(window, { staleKpiValues: failures });
		const observer = new MutationObserver(() => {
			const label = document.querySelector('.dashboard-toolbar button')?.getAttribute('aria-label');
			const value = document
				.querySelector('[data-kpi="flows"] [data-kpi-value]')
				?.getAttribute('data-kpi-value');
			if (label === 'Date range: Mar 1, 2025' && value && value !== '162') failures.push(value);
		});
		observer.observe(document.querySelector('.dashboard-toolbar')!.parentElement!, {
			childList: true,
			subtree: true,
			attributes: true,
			characterData: true
		});
	});
	await setDateRange(page, { startDate: '2025-03-01', endDate: '2025-03-01' });
	await expect(value).toHaveAttribute('data-kpi-value', '162');
	expect(await page.evaluate(() => Reflect.get(window, 'staleKpiValues'))).toEqual([]);
});
