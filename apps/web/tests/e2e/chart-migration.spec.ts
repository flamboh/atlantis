import { expect, test } from '@playwright/test';
import {
	FIXTURE_DASHBOARD,
	activateChart,
	chartCard,
	expectRendered,
	hoverChart,
	rendered
} from './chart-helpers';
import { intervalSelect, setDirection, setInterval } from './toolbar-helpers';

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
	await expect(intervalSelect(page)).toHaveAccessibleName('Interval: 5 min');
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
	for (const mode of ['dark', 'light']) {
		await page.getByRole('button', { name: `Switch to ${mode} mode` }).click();
		const canvas = surface.locator('canvas');
		await expect(canvas).toHaveCount(1);
		await expect(surface.locator('circle')).toHaveCount(0);
		const colors = await canvas.evaluate((element: HTMLCanvasElement) => {
			const pixels = element
				.getContext('2d')!
				.getImageData(0, 0, element.width, element.height).data;
			const hues = new Set<string>();
			let opaque = 0;
			for (let index = 0; index < pixels.length; index += 4) {
				if (pixels[index + 3] < 200) continue;
				opaque++;
				const [r, g, b] = [pixels[index], pixels[index + 1], pixels[index + 2]];
				hues.add(
					r > g && b > g
						? 'purple'
						: b > r && b > g
							? 'blue'
							: g > r && b > r
								? 'cyan'
								: g > r && g > b
									? 'green'
									: 'yellow'
				);
			}
			return { hues: [...hues], opaque };
		});
		expect(colors.opaque).toBeGreaterThan(100);
		expect(colors.hues).toEqual(
			expect.arrayContaining(['purple', 'blue', 'cyan', 'green', 'yellow'])
		);
	}

	await hoverChart(page, surface);
	await expect(card.getByRole('tooltip')).toContainText('2025-03-01 02:00');
	await surface.focus();
	await surface.press('Home');
	await expect(card.getByTestId('chart-tooltip')).toContainText('2025-03-01 02:00');
	await surface.press('Enter');
	await expect(page).toHaveURL(/\/netflow\/files\/202503010200/);
});

for (const width of [390, 768, 1280, 1920]) {
	test(`dense legends preserve positioned marks and axis labels at ${width}px`, async ({
		page
	}) => {
		await page.setViewportSize({ width, height: 900 });
		await page.route('**/api/ip/stats?*', async (route) => {
			const response = await route.fetch();
			const payload = await response.json();
			payload.timelines = [0, 1, 2].flatMap((index) =>
				payload.timelines.map((timeline: { router: string }) => ({
					...timeline,
					router: `${timeline.router}-${index}`
				}))
			);
			await route.fulfill({ response, json: payload });
		});
		await page.goto(FIXTURE_DASHBOARD);
		for (const id of ['ports', 'ip']) {
			const card = await activateChart(page, id);
			const surface = card.getByTestId('chart-surface');
			await rendered(surface);
			for (const series of await card.locator('[data-testid="chart-series"][data-point-y]').all()) {
				const y = Number(await series.getAttribute('data-point-y'));
				expect(y).toBeGreaterThan(0);
				expect(y).toBeLessThan(1);
			}
			const axis = surface.getByText('Time (5m)', { exact: true });
			await expect(axis).toBeVisible();
			await expect
				.poll(async () => {
					const graphic = await surface.boundingBox();
					const label = await axis.boundingBox();
					return Boolean(graphic && label && label.y + label.height <= graphic.y + graphic.height);
				})
				.toBe(true);
			const lastLegend = card.locator('[data-chart-legend-value]').last();
			await lastLegend.scrollIntoViewIfNeeded();
			const label = await lastLegend.getAttribute('data-chart-legend-value');
			const entry = card
				.getByTestId('chart-series')
				.filter({ hasText: label ?? '' })
				.last();
			await expect
				.poll(async () => {
					const graphic = await surface.boundingBox();
					const button = await lastLegend.boundingBox();
					if (!graphic || !button) return Infinity;
					return Math.abs(
						graphic.y +
							graphic.height * Number(await entry.getAttribute('data-legend-y')) -
							button.y -
							button.height / 2
					);
				})
				.toBeLessThan(1);
			await lastLegend.click();
			await expect(lastLegend).toHaveAttribute('aria-pressed', 'false');
			await lastLegend.press('Space');
			await expect(lastLegend).toHaveAttribute('aria-pressed', 'true');
			await hoverChart(page, surface);
			await expect(card.getByTestId('chart-tooltip')).toContainText('2025-03-01 02:00');
		}
	});
}

test('traffic stacks protocol values cumulatively and recomputes after legend toggles', async ({
	page
}) => {
	await page.goto(FIXTURE_DASHBOARD);
	const card = chartCard(page, 'dashboard');
	await rendered(card.getByTestId('chart-surface'));
	const positions = async () =>
		Promise.all(
			['TCP', 'UDP', 'ICMP', 'Other'].map(async (protocol) =>
				Number(
					await card
						.getByTestId('chart-series')
						.filter({ hasText: `Flows ${protocol}` })
						.getAttribute('data-point-y')
				)
			)
		);
	const y = await positions();
	for (let index = 1; index < y.length; index++) expect(y[index]).toBeLessThan(y[index - 1]);
	expect((y[0] - y[1]) / (y[1] - y[2])).toBeCloseTo(40 / 16, 5);
	expect((y[1] - y[2]) / (y[2] - y[3])).toBeCloseTo(16 / 4, 5);
	await card.getByRole('button', { name: 'Toggle Flows TCP series', exact: true }).click();
	await expect(card.getByTestId('chart-render-state')).toHaveAttribute('data-series-count', '3');
	const hidden = await positions();
	expect((hidden[1] - hidden[2]) / (hidden[2] - hidden[3])).toBeCloseTo(16 / 4, 5);
	await card.getByRole('button', { name: 'Toggle Flows TCP series', exact: true }).click();
	await expect.poll(positions).toEqual(y);
});

test('traffic aborts an obsolete request and retains the latest rendered grouping', async ({
	page
}) => {
	await page.goto(FIXTURE_DASHBOARD);
	const card = chartCard(page, 'dashboard');
	await rendered(card.getByTestId('chart-surface'));
	let release = () => {};
	const blocked = new Promise<void>((resolve) => {
		release = resolve;
	});
	await page.route('**/api/netflow/stats?*', async (route) => {
		if (new URL(route.request().url()).searchParams.get('groupBy') === 'hour') await blocked;
		await route.continue();
	});
	const requested = page.waitForRequest(
		(request) =>
			request.url().includes('/api/netflow/stats?') &&
			new URL(request.url()).searchParams.get('groupBy') === 'hour'
	);
	await setInterval(page, 'Hour');
	const pending = await requested;
	const aborted = page.waitForEvent('requestfailed', {
		predicate: (request) => request === pending
	});
	await setInterval(page, '10 min');
	await expect(card.getByTestId('chart-axis').filter({ hasText: '10 Minutes' })).toBeAttached();
	const tcp = card.getByTestId('chart-series').filter({ hasText: /^Flows TCP$/ });
	await expect(tcp).toHaveAttribute('data-total', '100');
	release();
	await aborted;
	await page.unrouteAll({ behavior: 'wait' });
	await expect(page).toHaveURL(/groupBy=10min/);
	await expect(card.getByTestId('chart-axis').filter({ hasText: '10 Minutes' })).toBeAttached();
	await expect(tcp).toHaveAttribute('data-total', '100');
});

test('filter transitions release detached chart DOM and listeners', async ({ page }) => {
	await page.goto(FIXTURE_DASHBOARD);
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
		await activateChart(page, id);
	}
	async function change(name: 'Ingress' | 'All') {
		await setDirection(page, name);
		await page.waitForLoadState('networkidle');
	}
	await change('Ingress');
	await change('All');
	const cdp = await page.context().newCDPSession(page);
	async function retainedDOM() {
		await cdp.send('HeapProfiler.collectGarbage');
		return cdp.send('Memory.getDOMCounters');
	}
	const before = await retainedDOM();
	for (let cycle = 0; cycle < 6; cycle++) {
		await change(cycle % 2 ? 'All' : 'Ingress');
	}
	const after = await retainedDOM();
	expect(after.nodes).toBeLessThanOrEqual(before.nodes + 100);
	expect(after.jsEventListeners).toBeLessThanOrEqual(before.jsEventListeners + 3);
});

for (const mode of ['light', 'dark']) {
	test(`router series retain distinct visible colors in ${mode} mode`, async ({ page }) => {
		await page.route(
			/\/api\/(ip\/stats|protocol\/stats|netflow\/dimension-stats|netflow\/characteristics)\?/,
			async (route) => {
				const response = await route.fetch();
				const payload = await response.json();
				if (payload.timelines)
					payload.timelines = [0, 1, 2].flatMap((index) =>
						payload.timelines.map((timeline: { router: string }) => ({
							...timeline,
							router: `${timeline.router}-${index}`
						}))
					);
				if (payload.portTimelines) {
					payload.portTimelines = [0, 1, 2].flatMap((index) =>
						payload.portTimelines.map((timeline: { sourceId: string }) => ({
							...timeline,
							sourceId: `${timeline.sourceId}-${index}`
						}))
					);
					payload.resolvedSources = payload.portTimelines.map(
						(timeline: { sourceId: string }) => timeline.sourceId
					);
				}
				await route.fulfill({ response, json: payload });
			}
		);

		await page.addInitScript(
			(dark) => localStorage.setItem('dark-mode', String(dark)),
			mode === 'dark'
		);
		await page.goto(FIXTURE_DASHBOARD);
		for (const id of ['dashboard', 'ports', 'ip', 'protocol', 'dimensions']) {
			const card = await activateChart(page, id);
			await expectRendered(card);
			const series = card.locator('[data-testid="chart-series"][data-visible="true"]');
			const appearance = await series.evaluateAll((entries) =>
				entries.map((entry) => ({
					color: entry.getAttribute('data-color'),
					opacity: Number(entry.getAttribute('data-opacity'))
				}))
			);
			expect(appearance.length).toBeGreaterThan(1);
			expect(new Set(appearance.map(({ color }) => color)).size).toBe(appearance.length);
			for (const { color, opacity } of appearance) {
				expect(color).toBeTruthy();
				expect(color).not.toBe('none');
				expect(color).not.toContain('var(');
				expect(opacity).toBeGreaterThan(0);
			}
		}
	});
}
