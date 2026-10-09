import { expect, type Locator, type Page } from '@playwright/test';

function escape(text: string): string {
	return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** Click a trigger until its popup appears; the first click can land before hydration. */
export async function openWith(trigger: Locator, popup: Locator): Promise<void> {
	await expect(async () => {
		if (!(await popup.isVisible())) await trigger.click();
		await expect(popup).toBeVisible({ timeout: 1000 });
	}).toPass({ timeout: 10_000 });
}

/** Open a toolbar popover by its trigger's accessible-name prefix and return the dialog. */
export async function openToolbarPopover(
	page: Page,
	trigger: RegExp,
	dialogName: string
): Promise<Locator> {
	const dialog = page.getByRole('dialog', { name: dialogName, exact: true });
	await openWith(page.getByRole('button', { name: trigger }), dialog);
	return dialog;
}

export async function closePopover(page: Page, dialog: Locator): Promise<void> {
	await page.keyboard.press('Escape');
	await expect(dialog).not.toBeAttached();
}

async function chooseSelectOption(page: Page, label: string, option: string) {
	await openWith(
		page.getByRole('button', { name: new RegExp(`^${escape(label)}:`) }),
		page.getByRole('listbox')
	);
	await page.getByRole('option', { name: new RegExp(`^${escape(option)}(\\s|$)`) }).click();
	await expect(page.getByRole('listbox')).not.toBeAttached();
}

export function intervalSelect(page: Page): Locator {
	return page.getByRole('button', { name: /^Interval:/ });
}

export async function setInterval(page: Page, option: string): Promise<void> {
	await chooseSelectOption(page, 'Interval', option);
	await expect(intervalSelect(page)).toHaveAccessibleName(`Interval: ${option}`);
}

export function directionSelect(page: Page): Locator {
	return page.getByRole('button', { name: /^Direction:/ });
}

export async function setDirection(page: Page, option: string): Promise<void> {
	await chooseSelectOption(page, 'Direction', option);
	await expect(directionSelect(page)).toHaveAccessibleName(`Direction: ${option}`);
}

export async function openMaadOptions(page: Page): Promise<Locator> {
	return openToolbarPopover(page, /^MAAD:/, 'MAAD options');
}

export async function setMaadMeasure(page: Page, measure: string): Promise<void> {
	const dialog = await openMaadOptions(page);
	const control = dialog.getByRole('group', { name: 'MAAD measure' });
	await control.getByRole('radio', { name: measure, exact: true }).click();
	await expect(control.getByRole('radio', { name: measure, exact: true })).toHaveAttribute(
		'aria-checked',
		'true'
	);
	await closePopover(page, dialog);
}

export async function setMaadFamily(page: Page, family: string): Promise<void> {
	const dialog = await openMaadOptions(page);
	const control = dialog.getByRole('group', { name: 'MAAD address family' });
	await control.getByRole('radio', { name: family }).click();
	await expect(control.getByRole('radio', { name: family })).toHaveAttribute(
		'aria-checked',
		'true'
	);
	await closePopover(page, dialog);
}

export async function openSources(page: Page): Promise<Locator> {
	return openToolbarPopover(page, /^Sources:/, 'Sources');
}

export function sourceOption(dialog: Locator, name: string): Locator {
	return dialog.getByRole('option', { name, exact: true });
}

export async function setSource(page: Page, name: string, enabled: boolean): Promise<void> {
	const dialog = await openSources(page);
	const option = sourceOption(dialog, name);
	if ((await option.getAttribute('aria-checked')) !== String(enabled)) await option.click();
	await expect(option).toHaveAttribute('aria-checked', String(enabled));
	await closePopover(page, dialog);
}

export async function openDateRange(page: Page): Promise<Locator> {
	return openToolbarPopover(page, /^Date range:/, 'Date range');
}

/** Type one or both ends of the range in the picker's fields; each field commits on change. */
export async function setDateRange(
	page: Page,
	range: { startDate?: string; endDate?: string }
): Promise<void> {
	const dialog = await openDateRange(page);
	for (const [label, value] of [
		['End Date', range.endDate],
		['Start Date', range.startDate]
	] as const) {
		if (value === undefined) continue;
		const field = dialog.getByLabel(label, { exact: true });
		await field.fill(value);
		await field.press('Tab');
	}
	await closePopover(page, dialog);
}

export async function resetFilters(page: Page): Promise<void> {
	await page.getByRole('button', { name: 'Reset filters', exact: true }).click();
}

export function cardMenuTrigger(page: Page, title: string): Locator {
	return page.getByRole('button', { name: `${title} card options`, exact: true });
}

export async function chooseCardAction(page: Page, title: string, action: string): Promise<void> {
	await openWith(cardMenuTrigger(page, title), page.getByRole('menu'));
	await page.getByRole('menuitem', { name: action, exact: true }).click();
	await expect(page.getByRole('menu')).not.toBeAttached();
}

/** Pick a segmented toggle (radio) inside a scope and assert it became checked. */
export async function chooseSegment(scope: Locator, group: string, option: string): Promise<void> {
	const radio = scope
		.getByRole('group', { name: group, exact: true })
		.getByRole('radio', { name: option, exact: true });
	await radio.click();
	await expect(radio).toHaveAttribute('aria-checked', 'true');
}
