import { expect, test } from '@playwright/test';
import {
	activateChart,
	chartCard,
	expectRendered,
	FIXTURE_DASHBOARD,
	hoverPlot,
	hoverChart,
	chartTarget,
	setDates
} from './chart-helpers';

test('renders every dashboard chart with series, finite marks and axis summaries', async ({
	page
}) => {
	await page.goto(FIXTURE_DASHBOARD);
	for (const [id, count, axis, legend] of [
		['dashboard', 1, 'Value', 'Flows TCP'],
		['characteristics', 2, 'Duration', 'Average duration'],
		['ports', 1, 'Unique ports', 'Source ports 0-1023'],
		['ip', 1, 'Unique IPs', 'fixture-router · Src IPv4'],
		['protocol', 1, 'Unique Protocols', 'fixture-router · IPv4'],
		['dimensions', 1, 'Source D1', 'fixture-router'],
		['spectrum', 1, 'alpha', 'Spectrum'],
		['coverage', 1, '', 'fixture-router']
	] as const) {
		const card = await activateChart(page, id);
		await expectRendered(card, count);
		if (axis)
			await expect(card.getByTestId('chart-axis').filter({ hasText: axis }).first()).toBeAttached();
		await expect(
			card.getByTestId('chart-series').filter({ hasText: legend }).first()
		).toHaveAttribute('data-visible', 'true');
	}
});

test('traffic metrics and family controls change the rendered values and series', async ({
	page
}) => {
	await page.goto(FIXTURE_DASHBOARD);
	const card = chartCard(page, 'dashboard');
	const state = (await expectRendered(card)).first();
	const tcp = () => card.getByTestId('chart-series').filter({ hasText: /^Flows TCP$/ });
	await expect(tcp()).toHaveAttribute('data-total', '100');
	await card.getByRole('button', { name: 'IPv4', exact: true }).click();
	await expect(tcp()).toHaveAttribute('data-total', '80');
	await card.getByRole('button', { name: 'IPv6', exact: true }).click();
	await expect(tcp()).toHaveAttribute('data-total', '20');
	await card.getByRole('button', { name: 'All', exact: true }).click();
	await expect(tcp()).toHaveAttribute('data-total', '100');
	for (const [label, kind] of [
		['Line Chart', 'line'],
		['Stacked Area', 'stacked']
	]) {
		await card.getByRole('button', { name: label, exact: true }).click();
		await expect(state).toHaveAttribute('data-kind', kind);
	}
	for (const [label, prefix, total] of [
		['Packets', 'Packets TCP', '1000'],
		['Bytes', 'Bytes TCP', '102400']
	]) {
		await card.getByRole('button', { name: label, exact: true }).click();
		await expect(
			card.getByTestId('chart-series').filter({ hasText: new RegExp(`^${prefix}$`) })
		).toHaveAttribute('data-total', total);
		await expect(state).toHaveAttribute('data-series-count', '4');
	}
	await card.getByRole('button', { name: 'Select All', exact: true }).click();
	await expect(state).toHaveAttribute('data-series-count', '12');
	await card.getByRole('button', { name: 'NetFlow metric series', exact: true }).click();
	const seriesControls = page.getByRole('dialog', { name: 'NetFlow metric series', exact: true });
	await expect(seriesControls.getByRole('checkbox')).toHaveCount(12);
	for (const checkbox of await seriesControls.getByRole('checkbox').all()) {
		await checkbox.uncheck();
	}
	await expect(state).toHaveAttribute('data-series-count', '0');
	await expect(state).toHaveAttribute('data-state', 'empty');
	await page.keyboard.press('Escape');
	await card.getByRole('button', { name: 'Flows', exact: true }).click();
	await expect(state).toHaveAttribute('data-series-count', '4');
	await hoverPlot(card.getByTestId('chart-surface'));
	await expect(card.getByTestId('chart-tooltip')).toContainText('Flows TCP: 100');
});

test('flow characteristics and port families retain their numerical meaning', async ({ page }) => {
	await page.goto(FIXTURE_DASHBOARD);
	const characteristics = await activateChart(page, 'characteristics');
	await expectRendered(characteristics, 2);
	const duration = characteristics
		.getByTestId('chart-series')
		.filter({ hasText: /^Average duration$/ });
	await expect(duration).toHaveAttribute('data-max', '1750');
	for (const [family, value] of [
		['IPv4', '2000'],
		['IPv6', '1000'],
		['All', '1750']
	]) {
		await characteristics.getByRole('radio', { name: family, exact: true }).check();
		await expect(duration).toHaveAttribute('data-max', value);
	}
	await hoverChart(page, characteristics.getByTestId('chart-surface').first());
	await expect(characteristics.getByTestId('chart-tooltip').first()).toContainText('1.75 s');
	const ports = await activateChart(page, 'ports');
	const state = (await expectRendered(ports)).first();
	const series = ports.getByTestId('chart-series').filter({ hasText: /^Source ports 0-1023$/ });
	await expect(series).toHaveAttribute('data-max', '8');
	await ports.getByRole('radio', { name: 'IPV6', exact: true }).check();
	await expect(series).toHaveAttribute('data-max', '2');
	for (const cb of await ports.getByRole('checkbox').all()) await cb.uncheck();
	await expect(ports.getByTestId('chart-card-state')).toHaveAttribute('data-state', 'no-metrics');
	await expect(ports).toContainText('Select at least one port range');
	await ports.getByRole('checkbox', { name: 'Source ports 0-1023', exact: true }).check();
	await expectRendered(ports);
	await expect(state).toHaveAttribute('data-series-count', '1');
});

test('IP and protocol toggles alter the visible series and recover from no metrics', async ({
	page
}) => {
	await page.goto(FIXTURE_DASHBOARD);
	for (const id of ['ip', 'protocol']) {
		const card = await activateChart(page, id);
		await expectRendered(card);
		const checkboxes = card.getByRole('checkbox');
		for (const cb of await checkboxes.all()) await cb.uncheck();
		await expect(card.getByTestId('chart-card-state')).toHaveAttribute('data-state', 'no-metrics');
		await expect(card).toContainText('Select at least one metric');
		await checkboxes.first().check();
		const state = (await expectRendered(card)).first();
		await expect(state).toHaveAttribute('data-series-count', '1');
		await expect(card.getByTestId('chart-series').first()).toHaveAttribute(
			'data-max',
			id === 'ip' ? '5' : '3'
		);
		await hoverPlot(card.getByTestId('chart-surface'));
		await expect(card.getByTestId('chart-tooltip')).toContainText('fixture-router');
	}
});

test('all granularities render and long windows guard expensive groupings', async ({ page }) => {
	await page.goto(FIXTURE_DASHBOARD);
	const card = chartCard(page, 'dashboard');
	await expectRendered(card);
	for (const [label, groupBy] of [
		['Day', 'date'],
		['Hour', 'hour'],
		['30 min', '30min'],
		['10 min', '10min'],
		['5 min', '5min']
	]) {
		await page
			.getByRole('group', { name: 'Granularity' })
			.getByRole('button', { name: label, exact: true })
			.click();
		await expect
			.poll(() => new URL(page.url()).searchParams.get('groupBy') ?? 'date')
			.toBe(groupBy);
		await expectRendered(card);
		await expect(
			card.getByTestId('chart-series').filter({ hasText: /^Flows TCP$/ })
		).toHaveAttribute('data-total', '100');
	}
	await setDates(page, '2025-01-01', '2025-12-31');
	await expect(page.getByRole('button', { name: '5 min', exact: true })).toBeDisabled();
	await expect(page.getByRole('button', { name: '10 min', exact: true })).toBeDisabled();
	await expect(page.getByRole('button', { name: '30 min', exact: true })).toBeDisabled();
	await expect(page.getByRole('button', { name: 'Day', exact: true })).toBeEnabled();
});

test('empty windows clear rendered data and source defaults recover after reload', async ({
	page
}) => {
	await page.goto(FIXTURE_DASHBOARD);
	const ip = await activateChart(page, 'ip');
	await expectRendered(ip);
	await setDates(page, '2020-01-01', '2020-01-02');
	await expect(ip.getByTestId('chart-card-state')).not.toHaveAttribute('data-state', 'loading');
	await expect(ip.getByTestId('chart-render-state')).not.toBeAttached();
	await setDates(page, '2025-03-01', '2025-03-01');
	await expectRendered(ip);
	await page.getByRole('checkbox', { name: 'fixture-router', exact: true }).uncheck();
	await expect(ip).toContainText('Select at least one source');
	await page.reload();
	await expect(page.getByRole('checkbox', { name: 'fixture-router', exact: true })).toBeChecked();
	await page.getByRole('checkbox', { name: 'fixture-router', exact: true }).check();
	await activateChart(page, 'ip');
	await expectRendered(ip);
	await expect(ip.getByTestId('chart-series').first()).toHaveAttribute('data-max', '5');
});

test('direction changes values and URL filters survive reload and history', async ({ page }) => {
	await page.goto(FIXTURE_DASHBOARD);
	const tcp = chartCard(page, 'dashboard')
		.getByTestId('chart-series')
		.filter({ hasText: /^Flows TCP$/ });
	await expect(tcp).toHaveAttribute('data-total', '100');
	const direction = page.getByRole('group', { name: 'Traffic direction' });
	await direction.getByRole('button', { name: 'Ingress', exact: true }).click();
	await expect(tcp).toHaveAttribute('data-total', '50');
	await page
		.getByRole('group', { name: 'MAAD measure' })
		.getByRole('button', { name: 'Bytes', exact: true })
		.click();
	await page
		.getByRole('group', { name: 'MAAD address family' })
		.getByRole('button', { name: 'IPv6 (/23–/64)' })
		.click();
	await page.reload();
	await expect(direction.getByRole('button', { name: 'Ingress', exact: true })).toHaveAttribute(
		'aria-pressed',
		'true'
	);
	await expect(
		page
			.getByRole('group', { name: 'MAAD measure' })
			.getByRole('button', { name: 'Bytes', exact: true })
	).toHaveAttribute('aria-pressed', 'true');
	await expect(tcp).toHaveAttribute('data-total', '50');
	await direction.getByRole('button', { name: 'All', exact: true }).click();
	await expect(tcp).toHaveAttribute('data-total', '100');
	await page.goBack();
	await expect(tcp).toHaveAttribute('data-total', '50');
	await page.goForward();
	await expect(tcp).toHaveAttribute('data-total', '100');
});

test('chart loading, request errors and recovery are visible', async ({ page }) => {
	let release = () => {};
	const blocked = new Promise<void>((resolve) => {
		release = resolve;
	});
	await page.route('**/api/ip/stats?**', async (route) => {
		await blocked;
		await route.fulfill({ status: 503, body: 'QA unavailable' });
	});
	await page.goto(FIXTURE_DASHBOARD);
	const ip = await activateChart(page, 'ip');
	await expect(ip.getByTestId('chart-card-state')).toHaveAttribute('data-state', 'loading');
	await expect(ip).toContainText('Loading IP data');
	release();
	await expect(ip.getByTestId('chart-card-state')).toHaveAttribute('data-state', 'error');
	await expect(ip).toContainText('QA unavailable');
	await page.unrouteAll({ behavior: 'wait' });
	await page.getByRole('button', { name: 'Hour', exact: true }).click();
	await expectRendered(ip);
});

test('brush selects a narrower daily range and reset restores defaults', async ({ page }) => {
	await page.goto('/datasets/playwright?startDate=2025-01-01&endDate=2025-03-31&groupBy=date');
	const card = chartCard(page, 'dashboard');
	await expectRendered(card);
	const surface = card.getByTestId('chart-surface');
	await surface.scrollIntoViewIfNeeded();
	const box = await surface.boundingBox();
	if (!box) throw new Error('Missing traffic plot');
	await page.mouse.move(box.x + box.width * 0.35, box.y + box.height * 0.5);
	await page.mouse.down();
	await page.mouse.move(box.x + box.width * 0.65, box.y + box.height * 0.5, { steps: 10 });
	await page.mouse.up();
	await expect.poll(() => new URL(page.url()).searchParams.get('groupBy')).not.toBe('date');
	await expect(page.getByLabel('Start Date', { exact: true })).not.toHaveValue('2025-01-01');
	await page.getByRole('button', { name: 'Reset View' }).click();
	await expect(page.getByLabel('Start Date', { exact: true })).toHaveValue('2025-03-01');
	await expectRendered(card);
});

for (const id of ['characteristics', 'ports', 'ip', 'protocol', 'dimensions']) {
	test(`${id} legend hides and restores the rendered series`, async ({ page }) => {
		await page.goto(FIXTURE_DASHBOARD);
		const card = await activateChart(page, id);
		await expectRendered(card, id === 'characteristics' ? 2 : 1);
		const surface = card.getByTestId('chart-surface').first();
		const series = surface.locator('..').getByTestId('chart-series').first();
		const target = await chartTarget(surface, 'legend');
		await page.mouse.click(target.x, target.y);
		await expect(series).toHaveAttribute('data-visible', 'false');
		await page.mouse.click(target.x, target.y);
		await expect(series).toHaveAttribute('data-visible', 'true');
	});
}

test('theme and navigation tip remain available by keyboard and after reload', async ({ page }) => {
	await page.goto(FIXTURE_DASHBOARD);
	await expectRendered(chartCard(page, 'dashboard'));
	const tip = page.getByRole('button', { name: 'Show navigation tip' });
	await tip.focus();
	await expect(tip).toBeFocused();
	await expect(tip).toHaveAttribute('title', /Drag across chart/);
	const theme = page.getByRole('button', { name: 'Switch to dark mode' });
	await theme.focus();
	await page.keyboard.press('Enter');
	await expect(page.getByRole('button', { name: 'Switch to light mode' })).toBeVisible();
	await expectRendered(chartCard(page, 'dashboard'));
	await page.reload();
	await expect(page.getByRole('button', { name: 'Switch to light mode' })).toBeVisible();
	await expectRendered(chartCard(page, 'dashboard'));
});
