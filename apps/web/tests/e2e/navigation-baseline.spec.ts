import { expect, test } from '@playwright/test';
import { expectRendered, FIXTURE_DASHBOARD, hoverChart } from './chart-helpers';

test('dataset picker and navigation open the selected dataset and alerts', async ({ page }) => {
	await page.goto('/');
	const fixture = page.getByRole('link', { name: 'Playwright Fixture', exact: true });
	const external = page.getByRole('link', { name: 'Playwright External MAAD', exact: true });
	await expect(fixture).toBeVisible();
	await expect(external).toBeVisible();
	await external.focus();
	await page.keyboard.press('Enter');
	await expect(page).toHaveURL(/\/datasets\/playwright-external-maad$/);
	await expect(
		page.getByRole('heading', { name: 'Playwright External MAAD', exact: true })
	).toHaveText('Playwright External MAAD');
	await page.getByRole('link', { name: 'Alerts', exact: true }).click();
	await expect(page).toHaveURL(/\/datasets\/playwright-external-maad\/alerts$/);
	await expect(page.getByRole('heading', { name: 'Singularity alerts' })).toBeVisible();
	await expect(page.getByText('The alert feed is not running', { exact: true })).toBeVisible();
	await expect(
		page.getByText('netflow-db feed playwright-external-maad', { exact: true })
	).toBeVisible();
	await page.getByRole('link', { name: 'Dashboard', exact: true }).click();
	await expect(page).toHaveURL(/\/datasets\/playwright-external-maad$/);
	await page.getByRole('link', { name: 'ATLANTIS', exact: true }).click();
	await fixture.click();
	await expect(page).toHaveURL(/\/datasets\/playwright$/);
});

test('file lookup validates input and Enter opens populated analysis', async ({ page }) => {
	await page.goto('/netflow/files');
	await page.getByRole('button', { name: 'Go to File' }).click();
	await expect(page.getByText('Please enter a timestamp')).toBeVisible();
	const timestamp = page.getByLabel('File Timestamp (YYYYMMDDHHmm)');
	await timestamp.fill('invalid');
	await timestamp.press('Enter');
	await expect(page.getByText('Invalid format. Expected 12 digits (YYYYMMDDHHmm)')).toBeVisible();
	await page.getByLabel('Dataset', { exact: true }).selectOption('playwright');
	await timestamp.fill('202503010200');
	await timestamp.press('Enter');
	await expect(page).toHaveURL(/\/netflow\/files\/202503010200\?dataset=playwright$/);
	await expect(
		page.getByRole('heading', { name: 'nfcapd.202503010200', exact: true })
	).toBeVisible();
	await expectRendered(page.locator('main'), 4);
	await expect(page.getByTestId('chart-axis').filter({ hasText: 'q' }).first()).toBeAttached();
	await expect(
		page
			.getByTestId('chart-series')
			.filter({ hasText: /^tau\(q\)$/ })
			.first()
	).toHaveAttribute('data-count', '33');
	await hoverChart(page, page.getByRole('img', { name: 'Structure function chart' }).first());
	await expect(
		page.getByTestId('chart-tooltip').filter({ hasText: 'tau(q)' }).first()
	).toContainText('1');
});

test('file next navigation keeps filters and explains absent MAAD analyses', async ({ page }) => {
	const requests: string[] = [];
	page.on('request', (request) => {
		if (/\/api\/netflow\/files\/.*\/details\?/.test(request.url())) requests.push(request.url());
	});
	await page.goto('/netflow/files/202503010200?dataset=playwright&measure=bytes');
	await expect(
		page.getByRole('group', { name: 'MAAD measure' }).getByRole('radio', { name: 'Bytes' })
	).toHaveAttribute('aria-checked', 'true');
	await expectRendered(page.locator('main'), 2);
	await expect.poll(() => requests.length).toBe(1);
	await expect(
		page.getByText('The spectrum is only computed for the addresses measure.')
	).toBeVisible();
	await page.getByRole('link', { name: 'Next File' }).click();
	await expect(page).toHaveURL(/\/netflow\/files\/202503010205\?dataset=playwright&measure=bytes$/);
	await expect(page.getByText('No source structure data.', { exact: true })).toBeVisible();
	await expect(page.getByText('No destination structure data.', { exact: true })).toBeVisible();
	await expect(page.getByRole('button', { name: 'Reload', exact: true })).toHaveCount(2);
	await expect.poll(() => requests.length).toBe(2);
	await page.goBack();
	await expectRendered(page.locator('main'), 2);
	await expect.poll(() => requests.length).toBe(3);
	await page.reload();
	await expectRendered(page.locator('main'), 2);
	await expect.poll(() => requests.length).toBe(4);
});

test('file summary request errors recover through Retry Summary', async ({ page }) => {
	let fail = true;
	await page.route('**/api/netflow/files/*/details?**', async (route) => {
		if (fail) await route.fulfill({ status: 503, json: { error: 'QA summary unavailable' } });
		else await route.continue();
	});
	await page.goto('/netflow/files/202503010200?dataset=playwright');
	await expect(page.getByText(/Failed to load file summary/)).toBeVisible();
	fail = false;
	await page.getByRole('button', { name: 'Retry Summary' }).click();
	await expectRendered(page.locator('main'), 4);
	await expect(page.getByText(/Failed to load file summary/)).not.toBeAttached();
});

for (const [path, status, title] of [
	['/datasets/unknown', '404', 'Page Not Found'],
	['/missing', '404', 'Page Not Found'],
	['/netflow/files/abc?dataset=playwright', '400', 'Bad Request']
]) {
	test(`error route ${path} offers working navigation`, async ({ page }) => {
		await page.goto(path);
		await expect(page.getByRole('heading', { name: status, exact: true })).toBeVisible();
		await expect(page.getByRole('heading', { name: title, exact: true })).toBeVisible();
		await page.getByRole('link', { name: 'Home', exact: true }).last().click();
		await expect(page.getByRole('link', { name: 'Playwright Fixture', exact: true })).toBeVisible();
	});
}

for (const width of [390, 1920]) {
	test(`main views and controls remain usable at ${width}px`, async ({ page }) => {
		await page.setViewportSize({ width, height: 900 });
		for (const path of [
			'/',
			'/netflow/files',
			'/datasets/playwright/alerts',
			FIXTURE_DASHBOARD,
			'/netflow/files/202503010200?dataset=playwright'
		]) {
			await page.goto(path);
			await expect(page.getByRole('link', { name: 'ATLANTIS', exact: true })).toBeVisible();
			await expect(page.getByRole('link', { name: 'Files', exact: true })).toBeVisible();
			if (path === FIXTURE_DASHBOARD)
				await expectRendered(page.locator('[data-chart-id="dashboard"]'));
			if (path.includes('/files/2025')) await expectRendered(page.locator('main'), 4);
			const widths = await page.evaluate(() => ({
				viewport: innerWidth,
				document: document.documentElement.scrollWidth
			}));
			expect(widths.document).toBeLessThanOrEqual(widths.viewport);
		}
		await page.goto('/netflow/files');
		const input = page.getByLabel('File Timestamp (YYYYMMDDHHmm)');
		await input.focus();
		await page.keyboard.press('Tab');
		await expect(page.getByRole('button', { name: 'Go to File' })).toBeFocused();
		await page.keyboard.press('Enter');
		await expect(page.getByText('Please enter a timestamp')).toBeVisible();
	});
}

test('file outside source bounds explains the failed summary and offers retry', async ({
	page
}) => {
	await page.goto('/netflow/files/202001010000?dataset=playwright');
	await expect(page.getByText(/Failed to load file summary/)).toBeVisible();
	await expect(page.getByRole('button', { name: 'Retry Summary' })).toBeVisible();
	await expect(page.getByTestId('chart-render-state')).not.toBeAttached();
});
