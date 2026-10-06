import { expect, test } from '@playwright/test';
import { activateChart, expectRendered, FIXTURE_DASHBOARD } from './chart-helpers';

test('MAAD dimensions render the selected side, order and weighted measure', async ({ page }) => {
	await page.goto(FIXTURE_DASHBOARD);
	const card = await activateChart(page, 'dimensions');
	await expectRendered(card);
	const series = card.getByTestId('chart-series').first();
	for (const [order, value] of [
		['D0', 0.92],
		['D1', 0.88],
		['D2', 0.85]
	] as const) {
		await card
			.getByRole('group', { name: 'MAAD dimension' })
			.getByRole('button', { name: order, exact: true })
			.click();
		await expect
			.poll(async () => Number(await series.getAttribute('data-max')))
			.toBeCloseTo(value, 5);
		await expect(
			card.getByTestId('chart-axis').filter({ hasText: `Source ${order}` })
		).toBeAttached();
	}
	await card
		.getByRole('group', { name: 'MAAD address side' })
		.getByRole('button', { name: 'Destination', exact: true })
		.click();
	await expect.poll(async () => Number(await series.getAttribute('data-max'))).toBeCloseTo(0.82, 5);
	await card.getByRole('button', { name: 'D1', exact: true }).click();
	for (const [measure, value] of [
		['Addresses', 0.86],
		['Packets', 0.69],
		['Bytes', 0.61]
	] as const) {
		await page
			.getByRole('group', { name: 'MAAD measure' })
			.getByRole('button', { name: measure, exact: true })
			.click();
		await expect
			.poll(async () => Number(await series.getAttribute('data-max')))
			.toBeCloseTo(value, 5);
	}
});
