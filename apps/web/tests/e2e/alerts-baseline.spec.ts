import { expect, test } from '@playwright/test';

test('alerts tail, horizon, sort and pagination change the rendered table', async ({ page }) => {
	await page.goto('/');
	await page.getByRole('link', { name: 'Playwright Fixture', exact: true }).click();
	await page.getByRole('link', { name: 'Alerts', exact: true }).click();
	await expect(page.getByText(/Live · last window/)).toBeVisible();
	const table = page.getByRole('region', { name: 'Anomalous addresses' });
	await expect(table.getByRole('row')).toHaveCount(101);
	await expect(table.getByRole('row').nth(1)).toContainText('198.51.100.105');
	await expect(table.getByRole('row').nth(1)).toContainText('2.105');
	await expect(page.getByText('showing 100 of 105')).toBeVisible();
	await page.getByRole('button', { name: 'Show more', exact: true }).click();
	await expect(table.getByRole('row')).toHaveCount(106);
	await expect(page.getByText('showing 105 of 105')).toBeVisible();
	await expect(page.getByRole('button', { name: 'Show more' })).not.toBeAttached();
	await page.getByRole('radio', { name: 'Recent', exact: true }).click();
	await expect(table.getByRole('row')).toHaveCount(101);
	await expect(table.getByRole('row').nth(1)).toContainText('198.51.100.104');
	for (const horizon of ['1h', '6h', '24h', '7d']) {
		await page.getByRole('radio', { name: horizon, exact: true }).click();
		await expect(page.getByRole('radio', { name: horizon, exact: true })).toHaveAttribute(
			'aria-checked',
			'true'
		);
		await expect(
			page.getByText(horizon === '1h' ? 'showing 100 of 104' : 'showing 100 of 105')
		).toBeVisible();
	}
	await page.getByRole('radio', { name: 'Low α', exact: true }).click();
	await expect(table.getByRole('row')).toHaveCount(2);
	await expect(table.getByRole('row').nth(1)).toContainText('203.0.113.7');
	await page.getByRole('radio', { name: '1h', exact: true }).click();
	await expect(page.getByText('No anomalous addresses in the last 1h.')).toBeVisible();
	await page.getByRole('radio', { name: 'High α', exact: true }).click();
	await expect(table.getByRole('row')).toHaveCount(101);
});

test('alerts show loading and retain the last table on a failed refresh', async ({ page }) => {
	await page.goto('/');
	await page.getByRole('link', { name: 'Playwright Fixture', exact: true }).click();
	await page.getByRole('link', { name: 'Alerts', exact: true }).click();
	const table = page.getByRole('region', { name: 'Anomalous addresses' });
	await expect(table.getByRole('row')).toHaveCount(101);
	let release = () => {};
	const pending = new Promise<void>((resolve) => {
		release = resolve;
	});
	await page.route('**/api/alerts?**', async (route) => {
		await pending;
		await route.fulfill({ status: 503, json: { data: null, error: 'QA feed unavailable' } });
	});
	await page.getByRole('radio', { name: 'Recent', exact: true }).click();
	await expect(table).toHaveAttribute('aria-busy', 'true');
	release();
	await expect(page.getByRole('alert')).toHaveText('QA feed unavailable');
	await expect(table.getByRole('row')).toHaveCount(101);
	await page.unrouteAll({ behavior: 'wait' });
	await page.getByRole('radio', { name: 'Most extreme', exact: true }).click();
	await expect(page.getByRole('alert')).not.toBeAttached();
	await expect(table).toHaveAttribute('aria-busy', 'false');
});

test('absent feed command copies and reports clipboard failures', async ({ page, context }) => {
	await context.grantPermissions(['clipboard-read', 'clipboard-write']);
	await page.goto('/datasets/playwright-external-maad/alerts');
	await page.getByRole('button', { name: 'Copy', exact: true }).click();
	await expect(page.getByRole('button', { name: 'Copied', exact: true })).toBeVisible();
	await expect
		.poll(() => page.evaluate(() => navigator.clipboard.readText()))
		.toBe('netflow-db feed playwright-external-maad');
	await page.evaluate(() => {
		Object.defineProperty(navigator.clipboard, 'writeText', {
			configurable: true,
			value: () => Promise.reject(new Error('denied'))
		});
	});
	await page.getByRole('button', { name: 'Copied', exact: true }).click();
	await expect(page.getByRole('alert')).toHaveText('Could not copy the command');
});
