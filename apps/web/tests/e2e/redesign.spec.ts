import { expect, test } from '@playwright/test';
import type { NetflowFileDetailsResponse } from '../../src/lib/types/types';
import { expectRendered, FIXTURE_DASHBOARD } from './chart-helpers';

test('keyboard layout controls reorder persistently and resize the rendered plot', async ({
	page
}) => {
	await page.goto(FIXTURE_DASHBOARD);
	const card = page.locator('[data-chart-id="dashboard"]');
	await expectRendered(card);
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
		.getByRole('button', { name: 'IPv6 (/23–/64)', exact: true })
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
			.getByRole('button', { name: 'IPv6 (/23–/64)', exact: true })
	).toHaveAttribute('aria-pressed', 'true');
	await rail.getByRole('checkbox', { name: 'fixture-router', exact: true }).check();
	await expectRendered(page.locator('[data-chart-id="dashboard"]'));
});

test('Escape cancels the first filter sheet open while its module is loading', async ({ page }) => {
	await page.setViewportSize({ width: 390, height: 900 });
	await page.goto(FIXTURE_DASHBOARD);
	await expectRendered(page.locator('[data-chart-id="dashboard"]'));
	let release!: () => void;
	let requested!: () => void;
	const pending = new Promise<void>((resolve) => (release = resolve));
	const started = new Promise<void>((resolve) => (requested = resolve));
	await page.route('**/_app/immutable/**/*.js', async (route) => {
		requested();
		await pending;
		await route.continue();
	});
	const trigger = page.getByRole('button', { name: 'Open filters', exact: true });
	try {
		await trigger.focus();
		await trigger.press('Enter');
		await started;
		await expect(trigger).toHaveAttribute('aria-expanded', 'true');
		await page.keyboard.press('Escape');
		await expect(trigger).toHaveAttribute('aria-expanded', 'false');
	} finally {
		release();
	}
	await page.waitForLoadState('networkidle');
	await expect(page.getByRole('dialog', { name: 'Filters', exact: true })).not.toBeAttached();
	await expect(trigger).toBeFocused();
	await trigger.click();
	await expect(page.getByRole('dialog', { name: 'Filters', exact: true })).toBeVisible();
	await page.keyboard.press('Escape');
	await expect(trigger).toBeFocused();
});

for (const width of [390, 1280]) {
	test(`metric disclosure supports independent selections and focus return at ${width}px`, async ({
		page
	}) => {
		await page.setViewportSize({ width, height: 900 });
		await page.goto(FIXTURE_DASHBOARD);
		const card = page.locator('[data-chart-id="dashboard"]');
		const state = (await expectRendered(card)).first();
		const trigger = card.getByRole('button', { name: 'NetFlow metrics', exact: true });
		await trigger.focus();
		await page.keyboard.press('Enter');
		const dialog = page.getByRole('dialog', { name: 'NetFlow metrics', exact: true });
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
		await dialog.getByRole('button', { name: 'Close netflow metrics', exact: true }).click();
		await expect(trigger).toBeFocused();
	});
}

test('large byte totals remain on one line in the 1280px file sidebar', async ({ page }) => {
	await page.setViewportSize({ width: 1280, height: 900 });
	await page.route('**/api/netflow/files/*/details?**', async (route) => {
		const response = await route.fetch();
		const body: NetflowFileDetailsResponse = await response.json();
		body.routers[0].summary.bytes = 160824538431;
		await route.fulfill({ response, json: body });
	});
	await page.goto('/netflow/files/202503010200?dataset=playwright');
	const total = page.getByText('Total: 160,824,538,431', { exact: true });
	await expect(total).toBeVisible();
	expect(
		await total.evaluate((node) => {
			const range = document.createRange();
			range.selectNodeContents(node);
			return range.getClientRects().length;
		})
	).toBe(1);
});

test('fractional file plots render once and follow rail and viewport resizing', async ({
	page
}) => {
	await page.addInitScript(() => {
		new MutationObserver((records) => {
			for (const record of records) {
				const node = record.target;
				if (node instanceof HTMLElement && node.dataset.testid === 'chart-render-state') {
					node.dataset.testRenderCount = String(Number(node.dataset.testRenderCount ?? 0) + 1);
				}
			}
		}).observe(document, {
			subtree: true,
			attributes: true,
			attributeFilter: ['data-series-count']
		});
	});
	await page.setViewportSize({ width: 1280, height: 900 });
	await page.goto('/netflow/files/202503010200?dataset=playwright');
	await expectRendered(page.locator('main'), 4);
	await page.evaluate(
		() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)))
	);
	const summaries = page.getByTestId('chart-render-state');
	for (const summary of await summaries.all()) {
		await expect(summary).toHaveAttribute('data-test-render-count', '1');
	}
	for (const name of ['Hide file options', 'Show file options']) {
		const before = await page.getByTestId('chart-surface').first().boundingBox();
		await page.getByRole('button', { name, exact: true }).click();
		await expect
			.poll(async () => (await page.getByTestId('chart-surface').first().boundingBox())?.width)
			.not.toBe(before?.width);
	}
	await page.setViewportSize({ width: 768, height: 900 });
	await expect(page.getByRole('button', { name: 'Open file options', exact: true })).toBeVisible();
	for (const surface of await page.getByTestId('chart-surface').all()) {
		await expect
			.poll(() =>
				surface.evaluate((node) => {
					if (!(node instanceof SVGSVGElement)) return Infinity;
					return Math.abs(node.viewBox.baseVal.width - node.parentElement!.clientWidth);
				})
			)
			.toBeLessThanOrEqual(1);
	}
});

test('phone quick-selection labels fit inside their separate buttons', async ({ page }) => {
	await page.setViewportSize({ width: 390, height: 900 });
	await page.goto(FIXTURE_DASHBOARD);
	await expectRendered(page.locator('[data-chart-id="dashboard"]'));
	for (const name of ['Select All', 'Select None']) {
		const button = page
			.locator('[data-chart-id="dashboard"]')
			.getByRole('button', { name, exact: true });
		await expect(button).toBeVisible();
		expect(
			await button.evaluate((node) => {
				const range = document.createRange();
				range.selectNodeContents(node);
				const bounds = node.getBoundingClientRect();
				return [...range.getClientRects()].every(
					(rect) => rect.left >= bounds.left && rect.right <= bounds.right
				);
			})
		).toBe(true);
		await button.click();
	}
	await expect(
		page.locator('[data-chart-id="dashboard"]').getByTestId('chart-render-state')
	).toHaveAttribute('data-state', 'empty');
});

for (const width of [390, 768, 1280, 1920]) {
	test(`toolbar controls align and fit at ${width}px`, async ({ page }) => {
		await page.setViewportSize({ width, height: 900 });
		await page.goto(FIXTURE_DASHBOARD);
		await expectRendered(page.locator('[data-chart-id="dashboard"]'));
		const toolbar = page.locator('.console-toolbar');
		const geometry = await toolbar.evaluate((node) => {
			const bounds = node.getBoundingClientRect();
			const rect = (selector: string) => {
				const element = node.querySelector(selector)!;
				const box = element.getBoundingClientRect();
				return { x: box.x, right: box.right, top: box.top, height: box.height, bottom: box.bottom };
			};
			return {
				left: bounds.left,
				right: bounds.right,
				start: rect('#startDate'),
				end: rect('#endDate'),
				toggle: rect('.rail-toggle'),
				reset: rect('.reset-view'),
				chips: Array.from(node.querySelectorAll('.filter-summary span'), (chip) => {
					const box = chip.getBoundingClientRect();
					return { left: box.left, right: box.right, top: box.top };
				})
			};
		});
		expect(geometry.start.height).toBe(36);
		for (const control of [geometry.end, geometry.toggle, geometry.reset]) {
			expect(control.height).toBe(geometry.start.height);
			expect(control.x).toBeGreaterThanOrEqual(geometry.left);
			expect(control.right).toBeLessThanOrEqual(geometry.right);
		}
		expect(geometry.end.top).toBe(geometry.start.top);
		expect(geometry.toggle.top).toBe(geometry.reset.top);
		if (width >= 640) expect(geometry.start.top).toBe(geometry.toggle.top);
		for (const chip of geometry.chips) {
			expect(chip.left).toBeGreaterThanOrEqual(geometry.left);
			expect(chip.right).toBeLessThanOrEqual(geometry.right);
			expect(chip.top).toBeGreaterThanOrEqual(geometry.start.bottom);
		}
	});
}

for (const width of [1280, 1920]) {
	test(`compact filter rail reserves plot width and room for three sources at ${width}px`, async ({
		page
	}) => {
		await page.setViewportSize({ width, height: 800 });
		await page.goto(FIXTURE_DASHBOARD);
		await expectRendered(page.locator('[data-chart-id="dashboard"]'));
		const rail = page.getByRole('complementary', { name: 'Filters', exact: true });
		const box = await rail.boundingBox();
		expect(box!.width).toBe(200);
		for (const name of ['Granularity', 'Traffic direction', 'MAAD measure']) {
			const group = rail.getByRole('group', { name, exact: true });
			const geometry = await group.evaluate((node) =>
				Array.from(node.querySelectorAll('button'), (button) => {
					const box = button.getBoundingClientRect();
					return { top: box.top, left: box.left, right: box.right, height: box.height };
				})
			);
			expect(new Set(geometry.map((button) => button.top)).size).toBeGreaterThan(1);
			for (const button of geometry) {
				expect(button.height).toBe(32);
				expect(button.left).toBeGreaterThanOrEqual(box!.x);
				expect(button.right).toBeLessThanOrEqual(box!.x + box!.width);
			}
		}
		const maad = await rail.getByRole('group', { name: 'MAAD options', exact: true }).boundingBox();
		const sources = await rail.locator('.router-filter label').all();
		for (const source of sources) expect((await source.boundingBox())!.height).toBe(32);
		const extraSourceRows = 3 - sources.length;
		expect(maad!.y + maad!.height + extraSourceRows * (32 + 4)).toBeLessThanOrEqual(800);
		const plot = await page.locator('[data-chart-id="dashboard"]').boundingBox();
		expect(plot!.width).toBeGreaterThan(width - 280);
	});
}

for (const width of [390, 768, 1280, 1920]) {
	for (const theme of ['light', 'dark'] as const) {
		test(`resized panels stay packed at ${width}px in ${theme}`, async ({ page }) => {
			await page.setViewportSize({ width, height: 900 });
			await page.emulateMedia({ colorScheme: theme });
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
						expect(card.top - geometry[index - 1].bottom).toBeCloseTo(16, 0);
						expect(card.left).toBe(geometry[0].left);
						expect(card.right).toBe(geometry[0].right);
					}
				}
			};
			for (const card of (await cards.all()).slice(0, -1)) {
				const shorter = card.getByRole('button', { name: /^Make .* shorter$/ });
				await shorter.click();
				await shorter.click();
				await expectPacked();
			}
			for (const card of (await cards.all()).slice(0, -1).reverse()) {
				await card.getByRole('button', { name: /^Make .* taller$/ }).click();
				await expectPacked();
			}
			await page.getByRole('button', { name: 'Move Traffic Overview down', exact: true }).click();
			await expectPacked();
		});
	}
}

test('native and keyboard resizing share the packed panel height', async ({ page }) => {
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
	await card.getByRole('button', { name: 'Make Traffic Overview shorter', exact: true }).focus();
	await page.keyboard.press('Enter');
	await expect
		.poll(() => frame.evaluate((node) => node.getBoundingClientRect().height))
		.toBe(initial.height - 180);
	await card.getByRole('button', { name: 'Make Traffic Overview taller', exact: true }).focus();
	await page.keyboard.press('Enter');
	await expect
		.poll(() => frame.evaluate((node) => node.getBoundingClientRect().height))
		.toBe(initial.height - 100);
	const gap = await page
		.locator('[data-chart-card]')
		.evaluateAll(
			(nodes) => nodes[1].getBoundingClientRect().top - nodes[0].getBoundingClientRect().bottom
		);
	expect(gap).toBe(16);
	await expectRendered(card);
});

test.describe('coarse pointer filter geometry', () => {
	test.use({ hasTouch: true });
	for (const width of [390, 1280]) {
		test(`keeps 44px filter targets at ${width}px`, async ({ page }) => {
			await page.setViewportSize({ width, height: 800 });
			await page.goto(FIXTURE_DASHBOARD);
			await expectRendered(page.locator('[data-chart-id="dashboard"]'));
			expect(await page.evaluate(() => matchMedia('(pointer: coarse)').matches)).toBe(true);
			if (width < 1024)
				await page.getByRole('button', { name: 'Open filters', exact: true }).click();
			const filters =
				width < 1024
					? page.getByRole('dialog', { name: 'Filters', exact: true })
					: page.getByRole('complementary', { name: 'Filters', exact: true });
			const heights = await filters
				.locator(".primary-filters [role='group'] button, .primary-filters .router-filter label")
				.evaluateAll((nodes) => nodes.map((node) => node.getBoundingClientRect().height));
			expect(heights.length).toBeGreaterThan(10);
			for (const height of heights) expect(height).toBeGreaterThanOrEqual(44);
		});
	}
});
