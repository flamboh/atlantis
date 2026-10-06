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

test('reversed dates explain the empty window and recover after correction', async ({ page }) => {
	await page.goto('/datasets/playwright?startDate=2025-03-02&endDate=2025-03-01');
	await expect(page.locator('[data-chart-id="dashboard"]')).toHaveAttribute(
		'data-chart-activated',
		'true'
	);
	await expect(page.getByRole('alert')).toHaveText('Start Date must be on or before End Date.');
	await page.getByLabel('Start Date', { exact: true }).fill('2025-03-01');
	await page.getByLabel('Start Date', { exact: true }).press('Tab');
	await expect(
		page.getByText('Start Date must be on or before End Date.', { exact: true })
	).toHaveCount(0);
	await expectRendered(page.locator('[data-chart-id="dashboard"]'));
});

for (const width of [390, 768]) {
	test(`filter sheet keeps selection and keyboard focus at ${width}px`, async ({ page }) => {
		await page.setViewportSize({ width, height: 900 });
		await page.goto(FIXTURE_DASHBOARD);
		await expectRendered(page.locator('[data-chart-id="dashboard"]'));
		const trigger = page.getByRole('button', { name: 'Open filters', exact: true });
		await trigger.focus();
		await page.keyboard.press('Enter');
		const sheet = page.getByRole('dialog', { name: 'Filters', exact: true });
		await expect(sheet).toBeVisible();
		await sheet
			.getByRole('group', { name: 'Traffic direction' })
			.getByRole('button', { name: 'Ingress', exact: true })
			.click();
		await expect.poll(() => new URL(page.url()).searchParams.get('direction')).toBe('ingress');
		await sheet.getByRole('checkbox', { name: 'fixture-router', exact: true }).uncheck();
		await page.keyboard.press('Escape');
		await expect(sheet).not.toBeAttached();
		await expect(trigger).toBeFocused();
		await expect(page.locator('[data-chart-id="dashboard"]')).toContainText(
			'Select at least one source'
		);
		await trigger.click();
		await expect(
			sheet.getByRole('checkbox', { name: 'fixture-router', exact: true })
		).not.toBeChecked();
		await sheet.getByRole('checkbox', { name: 'fixture-router', exact: true }).check();
		await sheet.getByRole('button', { name: 'Close filters', exact: true }).click();
		await expect(trigger).toBeFocused();
		await expectRendered(page.locator('[data-chart-id="dashboard"]'));
		await trigger.click();
		await page.setViewportSize({ width: 1280, height: 900 });
		await expect(page.getByRole('complementary', { name: 'Filters', exact: true })).toBeVisible();
		await expect(sheet).not.toBeAttached();
		await page.setViewportSize({ width, height: 900 });
		await expect(sheet).not.toBeAttached();
		await expect(trigger).toHaveAttribute('aria-expanded', 'false');
	});
}

test('collapsed desktop rail preserves sources and URL controls', async ({ page }) => {
	await page.goto(FIXTURE_DASHBOARD);
	await expectRendered(page.locator('[data-chart-id="dashboard"]'));
	const rail = page.getByRole('complementary', { name: 'Filters', exact: true });
	await rail
		.getByRole('group', { name: 'MAAD address family' })
		.getByRole('button', { name: 'IPv6', exact: true })
		.click();
	await rail.getByRole('checkbox', { name: 'fixture-router', exact: true }).uncheck();
	await page.getByRole('button', { name: 'Hide filters', exact: true }).click();
	await expect(rail).not.toBeAttached();
	await expect(page.locator('.filter-summary')).toContainText('0/1 sources');
	await expect(page.locator('.filter-summary')).toContainText('IPv6');
	await page.getByRole('button', { name: 'Show filters', exact: true }).click();
	await expect(
		rail.getByRole('checkbox', { name: 'fixture-router', exact: true })
	).not.toBeChecked();
	await expect(
		rail
			.getByRole('group', { name: 'MAAD address family' })
			.getByRole('button', { name: 'IPv6', exact: true })
	).toHaveAttribute('aria-pressed', 'true');
	await rail.getByRole('checkbox', { name: 'fixture-router', exact: true }).check();
	await expectRendered(page.locator('[data-chart-id="dashboard"]'));
});

for (const width of [390, 1280]) {
	test(`metric disclosure supports independent selections and focus return at ${width}px`, async ({
		page
	}) => {
		await page.setViewportSize({ width, height: 900 });
		await page.goto(FIXTURE_DASHBOARD);
		const card = page.locator('[data-chart-id="dashboard"]');
		const state = (await expectRendered(card)).first();
		const trigger = card.getByRole('button', { name: 'NetFlow metric series', exact: true });
		await trigger.focus();
		await page.keyboard.press('Enter');
		const dialog = page.getByRole('dialog', { name: 'NetFlow metric series', exact: true });
		await expect(dialog.getByRole('checkbox')).toHaveCount(12);
		await dialog.getByRole('checkbox', { name: 'Flows TCP', exact: true }).uncheck();
		await dialog.getByRole('checkbox', { name: 'Bytes UDP', exact: true }).check();
		await expect(state).toHaveAttribute('data-series-count', '4');
		await expect(
			card.getByTestId('chart-series').filter({ hasText: /^Bytes UDP$/ })
		).toHaveAttribute('data-visible', 'true');
		await page.keyboard.press('Escape');
		await expect(dialog).not.toBeAttached();
		await expect(trigger).toBeFocused();
		await trigger.click();
		await expect(
			dialog.getByRole('checkbox', { name: 'Flows TCP', exact: true })
		).not.toBeChecked();
		await expect(dialog.getByRole('checkbox', { name: 'Bytes UDP', exact: true })).toBeChecked();
		await dialog.getByRole('button', { name: 'Close netflow metric series', exact: true }).click();
		await expect(trigger).toBeFocused();
	});
}
