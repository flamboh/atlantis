import { expect, type Locator, type Page } from '@playwright/test';

export const FIXTURE_DASHBOARD =
	'/datasets/playwright?startDate=2025-03-01&endDate=2025-03-01&groupBy=5min';

export function chartCard(page: Page, id: string) {
	return page.locator(`[data-chart-id="${id}"]`);
}

export async function activateChart(page: Page, id: string) {
	const card = chartCard(page, id);
	await card.scrollIntoViewIfNeeded();
	await expect(card).toHaveAttribute('data-chart-activated', 'true');
	return card;
}

export async function expectRendered(scope: Locator, count = 1) {
	const states = scope.getByTestId('chart-render-state');
	await expect(states).toHaveCount(count);
	for (const state of await states.all()) {
		await expect(state).toHaveAttribute('data-state', 'ready');
		await expect
			.poll(async () => Number(await state.getAttribute('data-mark-count')))
			.toBeGreaterThan(0);
	}
	return states;
}

export async function hoverPlot(surface: Locator) {
	await surface.scrollIntoViewIfNeeded();
	const target = await chartTarget(surface);
	const box = await surface.boundingBox();
	if (!box) throw new Error('Missing chart bounds');
	await surface.hover({ position: { x: target.x - box.x, y: target.y - box.y } });
}

export async function setDates(page: Page, start: string, end: string) {
	for (const [label, date] of [
		['End Date', end],
		['Start Date', start]
	]) {
		await page.getByLabel(label, { exact: true }).fill(date);
		await page.getByLabel(label, { exact: true }).press('Tab');
		const key = label === 'End Date' ? 'endDate' : 'startDate';
		await expect
			.poll(
				() =>
					new URL(page.url()).searchParams.get(key) ??
					(key === 'startDate' ? '2025-03-01' : new Date().toISOString().slice(0, 10))
			)
			.toBe(date);
	}
	await expect(page.getByLabel('Start Date', { exact: true })).toHaveValue(start);
	await expect(page.getByLabel('End Date', { exact: true })).toHaveValue(end);
}

export async function rendered(surface: Locator) {
	await expect(surface).toBeVisible();
	await expect(surface).toHaveAttribute('data-chart-rendered', 'true');
	const states = await expectRendered(surface.locator('..'));
	return states.first();
}

export async function chartTarget(surface: Locator, kind: 'point' | 'legend' = 'point') {
	await surface.scrollIntoViewIfNeeded();
	const target = surface
		.locator('..')
		.locator(`[data-testid="chart-series"][data-${kind}-x]`)
		.first();
	await expect(target).toBeAttached();
	const x = Number(await target.getAttribute(`data-${kind}-x`));
	const y = Number(await target.getAttribute(`data-${kind}-y`));
	const bounds = await surface.boundingBox();
	if (!bounds) throw new Error('Missing chart surface bounds');
	return { x: bounds.x + bounds.width * x, y: bounds.y + bounds.height * y };
}

export async function hoverChart(page: Page, surface: Locator) {
	const point = await chartTarget(surface);
	await page.mouse.move(point.x, point.y);
}
