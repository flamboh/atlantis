import { expect, test } from '@playwright/test';
import {
	FIXTURE_DASHBOARD,
	activateChart,
	chartCard,
	expectRendered,
	hoverChart,
	rendered
} from './chart-helpers';

test('traffic tooltip totals describe the hovered bucket after trimming unknown bounds', async ({
	page
}) => {
	await page.goto(FIXTURE_DASHBOARD);
	const card = chartCard(page, 'dashboard');
	const surface = card.getByTestId('chart-surface');
	await rendered(surface);
	await hoverChart(page, surface);
	await expect(card.getByTestId('chart-tooltip')).toContainText('2025-03-01 02:00');
	await expect(card.getByTestId('chart-tooltip')).toContainText('Flows TCP: 100');
	await expect(card.getByTestId('chart-tooltip')).toContainText('Total Flows: 160');
});

test('single daily characteristic and port observations have finite positioned marks', async ({
	page
}) => {
	await page.goto('/datasets/playwright?startDate=2025-03-01&endDate=2025-03-01');
	for (const [id, count] of [
		['characteristics', 2],
		['ports', 1]
	] as const) {
		const card = await activateChart(page, id);
		await expectRendered(card, count);
		for (const target of await card.locator('[data-testid="chart-series"][data-point-x]').all()) {
			await expect(target).toHaveAttribute('data-count', '1');
			const x = Number(await target.getAttribute('data-point-x'));
			const y = Number(await target.getAttribute('data-point-y'));
			expect(x).toBeGreaterThan(0);
			expect(x).toBeLessThan(1);
			expect(Number.isFinite(y)).toBe(true);
		}
	}
});

test('keyboard users toggle legends, explore values, and open the focused bucket', async ({
	page
}) => {
	await page.goto(FIXTURE_DASHBOARD);
	const card = chartCard(page, 'dashboard');
	const surface = card.getByTestId('chart-surface');
	await rendered(surface);
	const toggle = card.getByRole('button', { name: 'Toggle Flows TCP series', exact: true });
	await toggle.focus();
	await toggle.press('Space');
	await expect(toggle).toHaveAttribute('aria-pressed', 'false');
	await toggle.press('Enter');
	await expect(toggle).toHaveAttribute('aria-pressed', 'true');
	await surface.focus();
	await surface.press('ArrowRight');
	await surface.press('Home');
	await expect(card.getByTestId('chart-tooltip')).toContainText('2025-03-01 02:00');
	await surface.press('Enter');
	await expect(page).toHaveURL(/\/netflow\/files\/202503010200/);
});

test('keyboard range selection changes the time window and Escape cancels it', async ({ page }) => {
	await page.goto('/datasets/playwright?startDate=2025-03-01&endDate=2025-03-03');
	const surface = chartCard(page, 'dashboard').getByTestId('chart-surface');
	await rendered(surface);
	await surface.focus();
	await surface.press('ArrowRight');
	await surface.press('Shift+ArrowRight');
	await surface.press('Escape');
	await expect(page).toHaveURL(/endDate=2025-03-03/);
	await surface.press('Shift+ArrowRight');
	await surface.press('Enter');
	await expect(page.getByRole('button', { name: '5 min', exact: true })).toHaveAttribute(
		'aria-pressed',
		'true'
	);
});

test('dense spectra retain one accessible interaction surface and positioned summaries', async ({
	page
}) => {
	await page.route('**/api/netflow/spectrum-stats?*', async (route) => {
		const response = await route.fetch();
		const payload = await response.json();
		for (const timeline of payload.timelines) {
			const bucket = timeline.buckets.find((bucket: { data: unknown }) => bucket.data);
			bucket.data.spectrumSa = Array.from({ length: 20001 }, (_, index) => ({
				alpha: 0.5 + index / 20000,
				f: index / 20000
			}));
		}
		await route.fulfill({ response, json: payload });
	});
	await page.goto(FIXTURE_DASHBOARD);
	const card = await activateChart(page, 'spectrum');
	const surface = card.getByTestId('chart-surface');
	await expect(surface).toHaveCount(1);
	await rendered(surface);
	await expect(card.getByTestId('chart-series')).toHaveAttribute('data-count', '20001');
	await hoverChart(page, surface);
	await expect(card.getByRole('tooltip')).toContainText('2025-03-01 02:00');
	await surface.focus();
	await surface.press('Home');
	await expect(card.getByTestId('chart-tooltip')).toContainText('2025-03-01 02:00');
	await surface.press('Enter');
	await expect(page).toHaveURL(/\/netflow\/files\/202503010200/);
});
