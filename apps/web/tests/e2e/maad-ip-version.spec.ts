import { expect, test } from '@playwright/test';

const IPV6_LABEL = 'IPv6 (/23–/64)';

test('keeps the IPv6 MAAD selection across drilldown, reload, and next file', async ({ page }) => {
	await page.goto('/datasets/playwright?startDate=2025-03-01&endDate=2025-03-01&groupBy=5min');

	const familyControl = page.getByRole('group', { name: 'MAAD address family' });
	const spectrum = page.locator('[data-chart-id="spectrum"]');
	await page.locator('[data-chart-sentinel="spectrum"]').scrollIntoViewIfNeeded();
	await expect(spectrum).toHaveAttribute('data-chart-activated', 'true');

	const spectrumRequest = page.waitForRequest(
		(request) =>
			new URL(request.url()).pathname === '/api/netflow/spectrum-stats' &&
			new URL(request.url()).searchParams.get('ipVersion') === '6'
	);
	await familyControl.getByRole('button', { name: IPV6_LABEL }).click();
	await expect.poll(() => new URL(page.url()).searchParams.get('ipVersion')).toBe('6');
	await spectrumRequest;

	await page.reload();
	await page.locator('[data-chart-sentinel="spectrum"]').scrollIntoViewIfNeeded();
	await expect(familyControl.getByRole('button', { name: IPV6_LABEL })).toHaveAttribute(
		'aria-pressed',
		'true'
	);

	const dashboard = page.locator('[data-chart-id="dashboard"]');
	await dashboard.scrollIntoViewIfNeeded();
	await dashboard.locator('canvas').first().click();
	await expect(page).toHaveURL(/\/netflow\/files\/\d{12}\?/);
	const fileUrl = new URL(page.url());
	expect(fileUrl.searchParams.get('dataset')).toBe('playwright');
	expect(fileUrl.searchParams.get('ipVersion')).toBe('6');

	const fileIpv6 = familyControl.getByRole('button', { name: IPV6_LABEL });
	await expect(fileIpv6).toHaveAttribute('aria-pressed', 'true');

	await page.reload();
	await expect(fileIpv6).toHaveAttribute('aria-pressed', 'true');

	await page
		.getByRole('group', { name: 'MAAD measure' })
		.getByRole('button', { name: 'Bytes' })
		.click();
	await expect.poll(() => new URL(page.url()).searchParams.get('measure')).toBe('bytes');
	expect(new URL(page.url()).searchParams.get('ipVersion')).toBe('6');

	const weightedFileUrl = page.url();
	await page.getByRole('link', { name: 'Next File' }).click();
	await expect(page).not.toHaveURL(weightedFileUrl);
	await expect.poll(() => new URL(page.url()).searchParams.get('ipVersion')).toBe('6');
	expect(new URL(page.url()).searchParams.get('dataset')).toBe('playwright');
	expect(new URL(page.url()).searchParams.get('measure')).toBe('bytes');
	await expect(fileIpv6).toHaveAttribute('aria-pressed', 'true');

	await page.getByRole('button', { name: 'IPv4 (/8–/24)' }).click();
	await expect.poll(() => new URL(page.url()).searchParams.get('ipVersion')).toBeNull();
	expect(new URL(page.url()).searchParams.get('measure')).toBe('bytes');
});

test('falls back to IPv4 MAAD for an unknown ipVersion on the dashboard', async ({ page }) => {
	await page.goto(
		'/datasets/playwright?startDate=2025-03-01&endDate=2025-03-01&groupBy=5min&ipVersion=5'
	);

	await expect(
		page
			.getByRole('group', { name: 'MAAD address family' })
			.getByRole('button', { name: 'IPv4 (/8–/24)' })
	).toHaveAttribute('aria-pressed', 'true');
});
