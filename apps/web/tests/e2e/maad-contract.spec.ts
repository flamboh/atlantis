import { expect, test } from '@playwright/test';
import { activateChart, expectRendered, FIXTURE_DASHBOARD } from './chart-helpers';
import { chooseSegment, setMaadMeasure } from './toolbar-helpers';

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
		await chooseSegment(card, 'MAAD dimension', order);
		await expect
			.poll(async () => Number(await series.getAttribute('data-max')))
			.toBeCloseTo(value, 5);
		await expect(
			card.getByTestId('chart-axis').filter({ hasText: `Source ${order}` })
		).toBeAttached();
	}
	await chooseSegment(card, 'MAAD address side', 'Destination');
	await expect.poll(async () => Number(await series.getAttribute('data-max'))).toBeCloseTo(0.82, 5);
	await chooseSegment(card, 'MAAD dimension', 'D1');
	for (const [measure, value] of [
		['Addresses', 0.86],
		['Packets', 0.69],
		['Bytes', 0.61]
	] as const) {
		await setMaadMeasure(page, measure);
		await expect
			.poll(async () => Number(await series.getAttribute('data-max')))
			.toBeCloseTo(value, 5);
	}
});
