import { expect, test, type Page } from '@playwright/test';
import type { NetflowStatsResponse, NetflowFileDetailsResponse } from '../../src/lib/types/types';
import { expectRendered, FIXTURE_DASHBOARD } from './chart-helpers';
import {
	cardMenuTrigger,
	chooseCardAction,
	openDateRange,
	openMaadOptions,
	openSources,
	sourceOption
} from './toolbar-helpers';

function cardOrder(page: Page) {
	return page
		.locator('[data-chart-card]')
		.evaluateAll((cards) => cards.map((card) => card.getAttribute('data-chart-id')));
}

test('KPI totals match the traffic window returned by the stats API', async ({ page }) => {
	const statsResponse = page.waitForResponse(
		(response) => new URL(response.url()).pathname === '/api/netflow/stats'
	);
	await page.goto('/datasets/playwright?startDate=2025-03-01&endDate=2025-03-03&groupBy=date');
	const stats = (await (await statsResponse).json()) as NetflowStatsResponse;
	const totals = { flows: 0, packets: 0, bytes: 0, complete: 0 };
	for (const bucket of stats.result) {
		if (bucket.coverage.state === 'complete') totals.complete += 1;
		if (!bucket.data) continue;
		totals.flows += bucket.data.flows;
		totals.packets += bucket.data.packets;
		totals.bytes += bucket.data.bytes;
	}
	expect(totals.flows).toBeGreaterThan(0);
	const kpis = page.getByRole('region', { name: 'Window totals' });
	for (const [id, value] of [
		['flows', totals.flows],
		['packets', totals.packets],
		['bytes', totals.bytes],
		['buckets', totals.complete]
	] as const) {
		await expect(kpis.locator(`[data-kpi="${id}"] [data-kpi-value]`)).toHaveAttribute(
			'data-kpi-value',
			String(value)
		);
	}
	await expect(kpis.locator('[data-kpi="buckets"]')).toContainText(
		`${totals.complete} / ${stats.result.length}`
	);
	await expect(kpis.locator('[data-kpi="flows"]')).toContainText(totals.flows.toLocaleString());
});

test('KPI totals follow source selection and show an unavailable state', async ({ page }) => {
	await page.goto(FIXTURE_DASHBOARD);
	const flows = page.locator('[data-kpi="flows"]');
	await expect(flows.locator('[data-kpi-value]')).toHaveAttribute('data-kpi-value', '160');
	const sources = await openSources(page);
	await sourceOption(sources, 'fixture-router').click();
	await expect(flows).toContainText('Unavailable');
	await sources.getByRole('button', { name: 'Select all', exact: true }).click();
	await expect(flows.locator('[data-kpi-value]')).toHaveAttribute('data-kpi-value', '160');
});

test('date range presets, calendar selection and typed fields update the window', async ({
	page
}) => {
	await page.goto('/datasets/playwright?startDate=2025-03-01&endDate=2025-03-03');
	await expect(page.getByRole('button', { name: 'Date range: Mar 1 – Mar 3, 2025' })).toBeVisible();
	let dialog = await openDateRange(page);
	await dialog.getByRole('button', { name: 'Last 7 days', exact: true }).click();
	await expect(dialog).not.toBeAttached();
	await expect.poll(() => new URL(page.url()).searchParams.get('startDate')).toBe('2025-02-25');
	await expect(
		page.getByRole('button', { name: 'Date range: Feb 25 – Mar 3, 2025' })
	).toBeFocused();

	dialog = await openDateRange(page);
	await expect(dialog.getByRole('button', { name: 'Last 7 days', exact: true })).toHaveAttribute(
		'aria-pressed',
		'true'
	);
	await dialog.getByRole('button', { name: /March 2, 2025/ }).click();
	await expect(dialog).toBeVisible();
	await dialog.getByRole('button', { name: /March 4, 2025/ }).click();
	await expect(dialog).not.toBeAttached();
	await expect.poll(() => new URL(page.url()).searchParams.get('startDate')).toBe('2025-03-02');
	expect(new URL(page.url()).searchParams.get('endDate')).toBe('2025-03-04');

	dialog = await openDateRange(page);
	const start = dialog.getByLabel('Start Date', { exact: true });
	await start.fill('2025-02-31');
	await start.press('Tab');
	await expect(start).toHaveAttribute('aria-invalid', 'true');
	expect(new URL(page.url()).searchParams.get('startDate')).toBe('2025-03-02');
	await start.fill('2025-03-01');
	await start.press('Tab');
	await expect.poll(() => new URL(page.url()).searchParams.get('startDate')).toBeNull();

	await dialog.getByRole('button', { name: 'All available', exact: true }).click();
	await expect.poll(() => new URL(page.url()).searchParams.get('endDate')).toBeNull();
	await expect(page.getByRole('button', { name: /^Date range: Mar 1, 2025 – / })).toBeVisible();
});

test('reversed dates explain the empty window and recover after correction', async ({ page }) => {
	await page.goto('/datasets/playwright?startDate=2025-03-02&endDate=2025-03-01');
	await expect(page.getByRole('alert')).toHaveText('Start Date must be on or before End Date.');
	const dialog = await openDateRange(page);
	await dialog.getByLabel('Start Date', { exact: true }).fill('2025-03-01');
	await dialog.getByLabel('Start Date', { exact: true }).press('Tab');
	await expect(
		page.getByText('Start Date must be on or before End Date.', { exact: true })
	).toHaveCount(0);
	await page.keyboard.press('Escape');
	await expectRendered(page.locator('[data-chart-id="dashboard"]'));
});

test('sources popover searches, toggles and bulk-selects with the keyboard', async ({ page }) => {
	await page.goto(FIXTURE_DASHBOARD);
	const trigger = page.getByRole('button', { name: /^Sources:/ });
	await expect(trigger).toHaveAccessibleName('Sources: 1 of 1 selected');
	const dialog = await openSources(page);
	const search = dialog.getByRole('combobox', { name: 'Search sources' });
	await expect(search).toBeFocused();
	await search.fill('nothing-matches');
	await expect(dialog.getByText('No sources found.')).toBeVisible();
	await search.fill('fixture');
	await expect(sourceOption(dialog, 'fixture-router')).toBeVisible();
	await page.keyboard.press('Enter');
	await expect(sourceOption(dialog, 'fixture-router')).toHaveAttribute('aria-checked', 'false');
	await expect(trigger).toHaveAccessibleName('Sources: 0 of 1 selected');
	await expect(page.locator('[data-chart-id="dashboard"]')).toContainText(
		'Select at least one source'
	);
	await dialog.getByRole('button', { name: 'Select all', exact: true }).click();
	await expect(sourceOption(dialog, 'fixture-router')).toHaveAttribute('aria-checked', 'true');
	await page.keyboard.press('Escape');
	await expect(dialog).not.toBeAttached();
	await expect(trigger).toBeFocused();
	await expectRendered(page.locator('[data-chart-id="dashboard"]'));
});

test('toolbar popovers and selects open, describe their value and return focus', async ({
	page
}) => {
	await page.goto(FIXTURE_DASHBOARD);
	await expect(page.getByRole('group', { name: 'Filters', exact: true })).toBeVisible();
	const maad = await openMaadOptions(page);
	await expect(maad.getByRole('group', { name: 'MAAD measure' })).toBeVisible();
	await expect(maad.getByRole('group', { name: 'MAAD address family' })).toBeVisible();
	await page.keyboard.press('Escape');
	await expect(page.getByRole('button', { name: 'MAAD: IPv4, Addresses' })).toBeFocused();

	const interval = page.getByRole('combobox', { name: 'Interval: 5 min' });
	await interval.focus();
	await page.keyboard.press('Enter');
	await expect(page.getByRole('listbox')).toBeVisible();
	await page.keyboard.press('Escape');
	await expect(interval).toBeFocused();

	const direction = page.getByRole('combobox', { name: 'Direction: All' });
	await direction.click();
	await expect(page.getByRole('option', { name: /^Egress/ })).toContainText('internal → external');
	await page.getByRole('option', { name: /^Egress/ }).click();
	await expect.poll(() => new URL(page.url()).searchParams.get('direction')).toBe('egress');
	await expect(page.getByRole('combobox', { name: 'Direction: Egress' })).toBeVisible();
});

for (const width of [390, 1280]) {
	test(`card menu moves and resizes cards with focus retained at ${width}px`, async ({ page }) => {
		await page.setViewportSize({ width, height: 900 });
		await page.goto(FIXTURE_DASHBOARD);
		const card = page.locator('[data-chart-id="dashboard"]');
		await expectRendered(card);
		const frame = card.locator('.chart-frame');
		const initial = await frame.evaluate((node) => node.getBoundingClientRect().height);
		await chooseCardAction(page, 'Traffic Overview', 'Taller');
		await expect
			.poll(() => frame.evaluate((node) => node.getBoundingClientRect().height))
			.toBe(initial + 80);
		await expect(cardMenuTrigger(page, 'Traffic Overview')).toBeFocused();
		await chooseCardAction(page, 'Traffic Overview', 'Shorter');
		await expect
			.poll(() => frame.evaluate((node) => node.getBoundingClientRect().height))
			.toBe(initial);
		await expectRendered(card);

		const trigger = cardMenuTrigger(page, 'Traffic Overview');
		await trigger.focus();
		await page.keyboard.press('Enter');
		const menu = page.getByRole('menu');
		await expect(menu.getByRole('menuitem', { name: 'Move up' })).toHaveAttribute(
			'aria-disabled',
			'true'
		);
		await menu.getByRole('menuitem', { name: 'Move down' }).focus();
		await page.keyboard.press('Enter');
		await expect(menu).not.toBeAttached();
		await expect
			.poll(() => cardOrder(page))
			.toEqual([
				'characteristics',
				'dashboard',
				'ports',
				'ip',
				'protocol',
				'dimensions',
				'spectrum',
				'coverage'
			]);
		await expect(trigger).toBeFocused();
		await page.reload();
		await expect.poll(async () => (await cardOrder(page))[0]).toBe('characteristics');
		await expect(page.locator('[data-chart-id="characteristics"]')).toHaveAttribute(
			'data-chart-activated',
			'true'
		);
		for (let position = 2; position < 8; position++) {
			await chooseCardAction(page, 'Traffic Overview', 'Move down');
			await expect.poll(async () => (await cardOrder(page))[position]).toBe('dashboard');
			await expect(trigger).toBeFocused();
		}
		await trigger.click();
		await expect(page.getByRole('menuitem', { name: 'Move down' })).toHaveAttribute(
			'aria-disabled',
			'true'
		);
		await page.keyboard.press('Escape');
		await expect(trigger).toBeFocused();
	});
}

test('menu and native resizing share the packed panel height', async ({ page }) => {
	await page.setViewportSize({ width: 1280, height: 1000 });
	await page.goto(FIXTURE_DASHBOARD);
	const card = page.locator('[data-chart-id="dashboard"]');
	await expectRendered(card);
	const frame = card.locator('.chart-frame');
	const initial = (await frame.boundingBox())!;
	await page.mouse.move(initial.x + initial.width - 3, initial.y + initial.height - 3);
	await page.mouse.down();
	await page.mouse.move(initial.x + initial.width - 3, initial.y + initial.height - 103, {
		steps: 5
	});
	await page.mouse.up();
	await expect
		.poll(() => frame.evaluate((node) => node.getBoundingClientRect().height))
		.toBe(initial.height - 100);
	await chooseCardAction(page, 'Traffic Overview', 'Shorter');
	await expect
		.poll(() => frame.evaluate((node) => node.getBoundingClientRect().height))
		.toBe(initial.height - 180);
	await chooseCardAction(page, 'Traffic Overview', 'Taller');
	await expect
		.poll(() => frame.evaluate((node) => node.getBoundingClientRect().height))
		.toBe(initial.height - 100);
	await expectRendered(card);
});

for (const width of [390, 1280]) {
	for (const theme of ['light', 'dark'] as const) {
		test(`resized panels stay packed at ${width}px in ${theme}`, async ({ page }) => {
			await page.setViewportSize({ width, height: 900 });
			await page.addInitScript(
				(dark) => localStorage.setItem('dark-mode', String(dark)),
				theme === 'dark'
			);
			await page.goto(FIXTURE_DASHBOARD);
			const cards = page.locator('[data-chart-card]');
			for (const card of await cards.all()) {
				await card.scrollIntoViewIfNeeded();
				await expect(card).toHaveAttribute('data-chart-activated', 'true');
				const id = await card.getAttribute('data-chart-id');
				await expectRendered(card, id === 'characteristics' ? 2 : 1);
			}
			const expectPacked = async () => {
				const geometry = await cards.evaluateAll((nodes) =>
					nodes.map((node) => {
						const card = node.getBoundingClientRect();
						const content = node.lastElementChild!.getBoundingClientRect();
						return {
							top: card.top,
							bottom: card.bottom,
							left: card.left,
							right: card.right,
							contentBottom: content.bottom
						};
					})
				);
				for (let index = 0; index < geometry.length; index++) {
					const card = geometry[index];
					expect(Math.abs(card.bottom - card.contentBottom)).toBeLessThanOrEqual(1);
					if (index > 0) {
						expect(card.top - geometry[index - 1].bottom).toBeCloseTo(12, 0);
						expect(card.left).toBe(geometry[0].left);
						expect(card.right).toBe(geometry[0].right);
					}
				}
			};
			for (const title of ['Traffic Overview', 'Unique Ports', 'Spectrum']) {
				await chooseCardAction(page, title, 'Shorter');
				await expectPacked();
				await chooseCardAction(page, title, 'Taller');
				await chooseCardAction(page, title, 'Taller');
				await expectPacked();
			}
			await chooseCardAction(page, 'Traffic Overview', 'Move down');
			await expectPacked();
		});
	}
}

test('placeholder cards expose the card menu before they load', async ({ page }) => {
	await page.goto(FIXTURE_DASHBOARD);
	const ip = page.locator('[data-chart-id="ip"]');
	await expect(ip).toHaveAttribute('data-chart-activated', 'false');
	await expect(ip.getByTestId('deferred-chart-ip')).toBeAttached();
	await cardMenuTrigger(page, 'Unique IP Counts').click();
	await expect(page.getByRole('menuitem', { name: 'Taller' })).not.toBeAttached();
	await page.getByRole('menuitem', { name: 'Move up' }).click();
	await expect.poll(async () => (await cardOrder(page)).indexOf('ip')).toBe(2);
});

test('top bar switches datasets and the command menu navigates', async ({ page }) => {
	await page.goto(FIXTURE_DASHBOARD);
	const switcher = page.getByRole('button', { name: 'Dataset: Playwright Fixture' });
	await switcher.click();
	await page.getByRole('combobox', { name: 'Search datasets' }).fill('external');
	await page.getByRole('option', { name: /Playwright External MAAD/ }).click();
	await expect(page).toHaveURL(/\/datasets\/playwright-external-maad$/);
	await expect(
		page.getByRole('button', { name: 'Dataset: Playwright External MAAD' })
	).toBeVisible();
	await expect(page.getByRole('link', { name: 'Dashboard', exact: true })).toHaveAttribute(
		'aria-current',
		'page'
	);

	await page.getByRole('link', { name: 'Alerts', exact: true }).click();
	await expect(page).toHaveURL(/\/datasets\/playwright-external-maad\/alerts$/);
	await page.getByRole('button', { name: 'Dataset: Playwright External MAAD' }).click();
	await page.getByRole('option', { name: /Playwright Fixture/ }).click();
	await expect(page).toHaveURL(/\/datasets\/playwright\/alerts$/);

	await page.keyboard.press('ControlOrMeta+k');
	const palette = page.getByRole('dialog', { name: 'Command menu' });
	await expect(palette).toBeVisible();
	await page.getByRole('combobox', { name: 'Search commands' }).fill('files');
	await page.keyboard.press('Enter');
	await expect(palette).not.toBeAttached();
	await expect(page).toHaveURL(/\/netflow\/files\?dataset=playwright$/);
	await expect(page.getByRole('link', { name: 'Files', exact: true })).toHaveAttribute(
		'aria-current',
		'page'
	);
	await expect(page.getByLabel('Dataset', { exact: true })).toHaveValue('playwright');

	await page.getByRole('button', { name: 'Open command menu' }).click();
	await page.getByRole('combobox', { name: 'Search commands' }).fill('dark theme');
	await page.keyboard.press('Enter');
	await expect(page.locator('html')).toHaveClass(/(?:^|\s)dark(?:\s|$)/);
});

for (const width of [390, 768, 1280, 1920]) {
	test(`toolbar controls wrap without overflow at ${width}px`, async ({ page }) => {
		await page.setViewportSize({ width, height: 900 });
		await page.goto(FIXTURE_DASHBOARD);
		await expectRendered(page.locator('[data-chart-id="dashboard"]'));
		const toolbar = page.getByRole('group', { name: 'Filters', exact: true });
		const geometry = await toolbar.evaluate((node) => {
			const bounds = node.getBoundingClientRect();
			return {
				left: bounds.left,
				right: bounds.right,
				controls: Array.from(node.querySelectorAll('button'), (button) => {
					const box = button.getBoundingClientRect();
					return { left: box.left, right: box.right, height: box.height };
				})
			};
		});
		expect(geometry.controls.length).toBe(6);
		for (const control of geometry.controls) {
			expect(control.height).toBe(32);
			expect(control.left).toBeGreaterThanOrEqual(geometry.left);
			expect(control.right).toBeLessThanOrEqual(geometry.right);
		}
		const widths = await page.evaluate(() => ({
			viewport: innerWidth,
			document: document.documentElement.scrollWidth
		}));
		expect(widths.document).toBeLessThanOrEqual(widths.viewport);
	});
}

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

test('large byte totals stay on one line in the file summary table', async ({ page }) => {
	await page.setViewportSize({ width: 1280, height: 900 });
	await page.route('**/api/netflow/files/*/details?**', async (route) => {
		const response = await route.fetch();
		const body: NetflowFileDetailsResponse = await response.json();
		body.routers[0].summary.bytes = 160824538431;
		await route.fulfill({ response, json: body });
	});
	await page.goto('/netflow/files/202503010200?dataset=playwright');
	const total = page.getByRole('cell', { name: '160,824,538,431', exact: true });
	await expect(total).toBeVisible();
	const fits = await total.evaluate((node) => {
		const table = node.closest('[data-slot="table-container"]') ?? node.closest('table')!;
		return (
			node.getBoundingClientRect().height < 40 &&
			node.getBoundingClientRect().right <= table.getBoundingClientRect().right + 1
		);
	});
	expect(fits).toBe(true);
});

test('metric popover keeps independent selections and returns focus', async ({ page }) => {
	await page.goto(FIXTURE_DASHBOARD);
	const card = page.locator('[data-chart-id="dashboard"]');
	const state = (await expectRendered(card)).first();
	const trigger = card.getByRole('button', { name: /^Metrics/ });
	await trigger.focus();
	await page.keyboard.press('Enter');
	const dialog = page.getByRole('dialog', { name: 'NetFlow metrics', exact: true });
	await expect(dialog.getByRole('checkbox')).toHaveCount(12);
	await dialog.getByRole('checkbox', { name: 'Flows TCP', exact: true }).uncheck();
	await dialog.getByRole('checkbox', { name: 'Bytes UDP', exact: true }).check();
	await expect(state).toHaveAttribute('data-series-count', '4');
	await expect(
		card.getByRole('group', { name: 'Traffic metric family' }).getByRole('radio', { checked: true })
	).toHaveCount(0);
	await page.keyboard.press('Escape');
	await expect(dialog).not.toBeAttached();
	await expect(trigger).toBeFocused();
	await expect(trigger).toContainText('4/12');
});

test.describe('coarse pointer targets', () => {
	test.use({ hasTouch: true });
	test('toolbar and card menu triggers keep 44px touch targets', async ({ page }) => {
		await page.setViewportSize({ width: 390, height: 800 });
		await page.goto(FIXTURE_DASHBOARD);
		await expectRendered(page.locator('[data-chart-id="dashboard"]'));
		expect(await page.evaluate(() => matchMedia('(pointer: coarse)').matches)).toBe(true);
		const heights = await page
			.locator('.toolbar-trigger, [data-card-menu-trigger]')
			.evaluateAll((nodes) => nodes.map((node) => node.getBoundingClientRect().height));
		expect(heights.length).toBeGreaterThan(8);
		for (const height of heights) expect(height).toBeGreaterThanOrEqual(44);
	});
});
