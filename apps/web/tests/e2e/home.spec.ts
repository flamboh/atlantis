import { expect, test } from '@playwright/test';

test('home shell renders core navigation', async ({ page }) => {
	await page.goto('/');

	await expect(page.getByRole('link', { name: 'ATLANTIS' })).toBeVisible();
	await expect(page.getByRole('link', { name: 'Datasets', exact: true })).toHaveAttribute(
		'aria-current',
		'page'
	);
	await expect(page.getByRole('link', { name: 'Files' })).toBeVisible();
	await expect(page).toHaveTitle(/ATLANTIS/i);
});
