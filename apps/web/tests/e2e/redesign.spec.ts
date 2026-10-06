import { expect, test } from '@playwright/test';
import { expectRendered, FIXTURE_DASHBOARD } from './chart-helpers';

test('keyboard layout controls reorder persistently and resize the rendered plot', async ({
	page
}) => {
	await page.goto(FIXTURE_DASHBOARD);
	const card = page.locator('[data-chart-id="dashboard"]');
	await expectRendered(card);
	const layout = card.locator('summary');
	await layout.focus();
	await page.keyboard.press('Enter');
	const frame = card.locator('.chart-frame');
	const initial = await frame.evaluate((node) => node.getBoundingClientRect().height);
	await page.getByRole('button', { name: 'Make Traffic Overview taller' }).focus();
	await page.keyboard.press('Enter');
	await expect
		.poll(() => frame.evaluate((node) => node.getBoundingClientRect().height))
		.toBe(initial + 80);
	await expectRendered(card);
	await page.getByRole('button', { name: 'Make Traffic Overview shorter' }).focus();
	await page.keyboard.press('Enter');
	await expect
		.poll(() => frame.evaluate((node) => node.getBoundingClientRect().height))
		.toBe(initial);
	await page.getByRole('button', { name: 'Move Traffic Overview down' }).focus();
	await page.keyboard.press('Enter');
	await expect(page.locator('[data-chart-id]').first()).toHaveAttribute(
		'data-chart-id',
		'characteristics'
	);
	await page.reload();
	await expect(page.locator('[data-chart-id]').first()).toHaveAttribute(
		'data-chart-id',
		'characteristics'
	);
	await expect(page.locator('[data-chart-id="characteristics"]')).toHaveAttribute(
		'data-chart-activated',
		'true'
	);
});

for (const width of [390, 768, 1280, 1920]) {
	test(`file summary and analysis labels fit at ${width}px`, async ({ page }) => {
		await page.setViewportSize({ width, height: 900 });
		await page.goto('/netflow/files/202503010200?dataset=playwright');
		await expectRendered(page.locator('main'), 4);
		await expect(page).toHaveTitle('nfcapd.202503010200 · ATLANTIS');
		for (const side of ['Source', 'Destination']) {
			for (const kind of ['Structure', 'Spectrum']) {
				const heading = page
					.getByRole('heading', { name: `${side} · ${kind}`, exact: true })
					.first();
				await heading.scrollIntoViewIfNeeded();
				await expect(heading).toBeVisible();
			}
		}
		const shell = page.locator('.app-shell__main');
		expect(await shell.evaluate((node) => node.scrollWidth - node.clientWidth)).toBeLessThanOrEqual(
			1
		);
	});
}
