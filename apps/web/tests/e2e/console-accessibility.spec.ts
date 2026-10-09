import { expect, test } from '@playwright/test';
import { FIXTURE_DASHBOARD, expectRendered } from './chart-helpers';
import { cardMenuTrigger, openSources, openWith } from './toolbar-helpers';

test('Tab leaves the card menu and follows the visible header order', async ({ page }) => {
	await page.setViewportSize({ width: 390, height: 900 });
	await page.goto(FIXTURE_DASHBOARD);
	const card = page.locator('[data-chart-id="dashboard"]');
	await expectRendered(card);
	const trigger = cardMenuTrigger(page, 'Traffic Overview');
	await trigger.focus();
	await page.keyboard.press('Enter');
	await expect(page.getByRole('menu')).toBeVisible();
	await page.keyboard.press('Shift+Tab');
	await expect(page.getByRole('menu')).not.toBeAttached();
	await expect(card.getByRole('radio', { name: 'Stacked', exact: true })).toBeFocused();
	const positions = await card.locator('.chart-card-header button').evaluateAll((nodes) =>
		nodes.map((node) => {
			const box = node.getBoundingClientRect();
			return { x: box.x, y: box.y + box.height / 2 };
		})
	);
	for (let index = 1; index < positions.length; index++) {
		const previous = positions[index - 1];
		const current = positions[index];
		expect(
			current.y > previous.y + 1 ||
				(Math.abs(current.y - previous.y) <= 1 && current.x >= previous.x)
		).toBe(true);
	}
});

test('a late source-list chunk preserves focus after interacting with the loading popover', async ({
	page
}) => {
	let release!: () => void;
	let started!: () => void;
	const held = new Promise<void>((resolve) => (release = resolve));
	const requested = new Promise<void>((resolve) => (started = resolve));
	await page.route('**/_app/immutable/**/*.js', async (route) => {
		const response = await route.fetch();
		const body = await response.text();
		if (body.includes('Search sources')) {
			started();
			await held;
		}
		await route.fulfill({ response, body });
	});
	try {
		await page.goto(FIXTURE_DASHBOARD);
		await expectRendered(page.locator('[data-chart-id="dashboard"]'));
		const dialog = await openSources(page);
		await requested;
		const clear = dialog.getByRole('button', { name: 'Clear', exact: true });
		await clear.click();
		await expect(clear).toBeFocused();
		release();
		await expect(dialog.getByRole('combobox', { name: 'Search sources' })).toBeVisible();
		await expect(clear).toBeFocused();
		await page.keyboard.press('Escape');
		await expect(page.getByRole('button', { name: /^Sources:/ })).toBeFocused();
	} finally {
		release();
	}
});

test('dataset popover is named and restores focus when cancelled', async ({ page }) => {
	await page.goto(FIXTURE_DASHBOARD);
	const trigger = page.getByRole('button', { name: 'Dataset: Playwright Fixture' });
	await trigger.focus();
	const dialog = page.getByRole('dialog', { name: 'Choose dataset', exact: true });
	await openWith(trigger, dialog);
	await expect(dialog.getByRole('combobox', { name: 'Search datasets' })).toBeFocused();
	await page.keyboard.press('Escape');
	await expect(trigger).toBeFocused();
});

test('moving a deferred card restores focus through activation and data loading', async ({
	page
}) => {
	await page.addInitScript(() => {
		Object.defineProperty(window, 'IntersectionObserver', {
			value: class {
				observe() {}
				unobserve() {}
				disconnect() {}
			}
		});
	});
	await page.goto(FIXTURE_DASHBOARD);
	await expectRendered(page.locator('[data-chart-id="dashboard"]'));
	const card = page.locator('[data-chart-id="ip"]');
	await expect(card).toHaveAttribute('data-chart-activated', 'false');
	await openWith(cardMenuTrigger(page, 'Unique IP Counts'), page.getByRole('menu'));
	await expect(card).toHaveAttribute('data-chart-activated', 'false');
	await page.getByRole('menuitem', { name: 'Move up', exact: true }).focus();
	await page.keyboard.press('Enter');
	await expect(card).toHaveAttribute('data-chart-activated', 'true');
	await expect(cardMenuTrigger(page, 'Unique IP Counts')).toBeFocused();
	await expectRendered(card);
	await expect(cardMenuTrigger(page, 'Unique IP Counts')).toBeFocused();
});
