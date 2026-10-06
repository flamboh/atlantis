import { expect, test } from '@playwright/test';

for (const colorScheme of ['light', 'dark'] as const) {
	test(`brand is accessible and its icons resolve in ${colorScheme} mode`, async ({ page }) => {
		await page.emulateMedia({ colorScheme });
		await page.setViewportSize({ width: 320, height: 800 });
		await page.goto('/netflow/files');
		const home = page.getByRole('link', { name: 'ATLANTIS', exact: true });
		await expect(home.getByRole('img', { name: 'ATLANTIS', exact: true })).toBeVisible();
		for (const control of await page.locator('header a, header button').all()) {
			const bounds = await control.boundingBox();
			expect(bounds).not.toBeNull();
			expect(bounds!.x).toBeGreaterThanOrEqual(0);
			expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(320);
		}
		await home.focus();
		await page.keyboard.press('Enter');
		await expect(page).toHaveURL(/\/$/);
		for (const selector of [
			'link[rel="icon"][type="image/svg+xml"]',
			'link[rel="icon"][type="image/x-icon"]',
			'link[rel="apple-touch-icon"]'
		]) {
			const href = await page.locator(selector).getAttribute('href');
			expect(href).toBeTruthy();
			const response = await page.request.get(href!);
			expect(response.status()).toBe(200);
			expect(response.headers()['content-type']).toMatch(/image\//);
			expect((await response.body()).length).toBeGreaterThan(0);
		}
	});
}
