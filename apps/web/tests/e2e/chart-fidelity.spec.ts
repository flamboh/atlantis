import { expect, test, type Page } from '@playwright/test';
import { activateChart, expectRendered } from './chart-helpers';

test('time axes keep midnight and noon labels with bounded grid geometry', async ({ page }) => {
	await page.route('**/api/ip/stats?*', async (route) => {
		const response = await route.fetch();
		const payload = await response.json();
		for (const timeline of payload.timelines) {
			const bucket = timeline.buckets.find((bucket: { data: unknown }) => bucket.data);
			timeline.buckets = Array.from({ length: 288 }, (_, index) => ({
				...bucket,
				bucketStart: 1740816000 + index * 600,
				bucketEnd: 1740816000 + (index + 1) * 600
			}));
		}
		await route.fulfill({ response, json: payload });
	});
	await page.goto('/datasets/playwright?startDate=2025-03-01&endDate=2025-03-02&groupBy=10min');
	const card = await activateChart(page, 'ip');
	await expectRendered(card);
	const surface = card.getByTestId('chart-surface');
	for (const label of ['Sat 3/1 00:00', 'Sat 3/1 12:00', 'Sun 3/2 00:00', 'Sun 3/2 12:00']) {
		await expect(surface.locator('text').filter({ hasText: label })).toBeVisible();
	}
	await expect(surface.locator('path[data-ts-key^="temporal-grid-"]')).toHaveCount(2);
	const ticks = surface.locator('text').filter({ hasText: /3\/[12] (00|12):00/ });
	for (const tick of await ticks.all())
		await expect(tick).toHaveAttribute('transform', /rotate\(-45/);
});

for (const mode of ['light', 'dark']) {
	test(`file analysis preserves numeric axes and point styles in ${mode} mode`, async ({
		page
	}) => {
		await page.addInitScript(
			(dark) => localStorage.setItem('dark-mode', String(dark)),
			mode === 'dark'
		);
		await page.goto('/netflow/files/202503010200?dataset=playwright');
		const spectrum = page.getByRole('img', { name: 'Multifractal spectrum chart' }).first();
		await expect(spectrum).toHaveAttribute('data-chart-rendered', 'true');
		const points = spectrum.locator('circle[data-ts-key^="series-0-dots"]');
		await expect(points.first()).toBeAttached();
		const styles = await points.evaluateAll((points) =>
			points.map((point) => ({
				fill: getComputedStyle(point).fill,
				stroke: getComputedStyle(point).stroke,
				width: getComputedStyle(point).strokeWidth
			}))
		);
		for (const style of styles) {
			expect(style.fill).not.toBe(style.stroke);
			expect(style.width).toBe('1px');
		}
		const numericLabels = spectrum.locator('text').filter({ hasText: /^0\.[0-9]/ });
		expect(await numericLabels.count()).toBeGreaterThan(0);
		for (const label of await numericLabels.all())
			expect((await label.getAttribute('transform')) ?? '').not.toMatch(/rotate\(-(30|45)/);
		const line = spectrum.locator('path[data-ts-key^="series-0-run"]').first();
		await expect(line).toHaveAttribute('stroke-width', '2');
		await expect(spectrum.locator('path[data-ts-key^="annotation-"]').first()).toHaveAttribute(
			'stroke-opacity',
			'0.5'
		);
		const swatch = spectrum
			.locator('..')
			.getByRole('button', { name: 'Toggle f(alpha) series' })
			.locator('[data-chart-legend-swatch] svg');
		await expect(swatch).toBeAttached();
		await expect(swatch.locator('circle')).toHaveAttribute('r', '3');
		const structure = page.getByRole('img', { name: 'Structure function chart' }).first();
		await expect(structure).toHaveAttribute('data-chart-rendered', 'true');
		await expect(structure.locator('text').filter({ hasText: /^-2\.1$/ })).toBeAttached();
		await expect(structure.locator('text').filter({ hasText: /^4\.1$/ })).toBeAttached();
	});
}

async function expectLocalClipPaths(page: Page) {
	const clips = await page.getByTestId('chart-surface').evaluateAll((surfaces) =>
		surfaces.flatMap((surface) =>
			Array.from(surface.querySelectorAll('g[clip-path]')).map((group) => {
				const id = group.getAttribute('clip-path')?.match(/^url\(#(.+)\)$/)?.[1];
				const local = Array.from(surface.querySelectorAll('clipPath')).find(
					(clip) => clip.id === id
				);
				return {
					id,
					local: Boolean(local),
					resolvedLocally: Boolean(local && document.getElementById(id ?? '')?.isSameNode(local))
				};
			})
		)
	);
	expect(clips.length).toBeGreaterThan(1);
	expect(new Set(clips.map((clip) => clip.id)).size).toBe(clips.length);
	for (const clip of clips) {
		expect(clip.local).toBe(true);
		expect(clip.resolvedLocally).toBe(true);
	}
}

test('each dashboard and file chart resolves its own clipping rectangle', async ({ page }) => {
	await page.goto('/datasets/playwright?startDate=2025-03-01&endDate=2025-03-02&groupBy=10min');
	for (const id of [
		'dashboard',
		'characteristics',
		'ports',
		'ip',
		'protocol',
		'dimensions',
		'spectrum',
		'coverage'
	]) {
		const card = await activateChart(page, id);
		await expectRendered(card, id === 'characteristics' ? 2 : 1);
	}
	await expectLocalClipPaths(page);
	await page.getByRole('button', { name: 'Switch to dark mode' }).click();
	await expect(page.locator('html')).toHaveClass(/dark/);
	await expectLocalClipPaths(page);
	await page.goto('/netflow/files/202503010200?dataset=playwright');
	await expect(page.getByTestId('chart-surface').first()).toHaveAttribute(
		'data-chart-rendered',
		'true'
	);
	await expectLocalClipPaths(page);
});
