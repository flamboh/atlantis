import { chromium } from '@playwright/test';

const baseUrl = process.env.ATLANTIS_PERF_BASE_URL ?? 'http://127.0.0.1:4173';
const latencyMs = Number(process.env.ATLANTIS_PERF_LATENCY_MS ?? 80);
const downloadKibPerSecond = Number(process.env.ATLANTIS_PERF_DOWNLOAD_KIB_PER_SECOND ?? 0);
const url = new URL('/datasets/performance', baseUrl);
url.search = new URLSearchParams({
	startDate: '2025-02-01',
	endDate: '2025-02-02',
	groupBy: 'hour'
}).toString();

const loadingCopy = [
	'Loading data',
	'Loading flow characteristics',
	'Loading IP data',
	'Loading protocol data',
	'Loading spectrum data',
	'Loading coverage'
];

async function waitForDashboard(page) {
	await page.waitForFunction(
		(copy) => copy.every((text) => !document.body.innerText.includes(text)),
		loadingCopy,
		{ timeout: 60_000 }
	);
	await page.evaluate(
		() =>
			new Promise((resolve) =>
				requestAnimationFrame(() => requestAnimationFrame(() => resolve(undefined)))
			)
	);
}

async function setEndDate(page, value) {
	const input = page.locator('#endDate');
	await input.fill(value);
	await input.press('Tab');
	await page.waitForFunction(
		(expected) => new URL(location.href).searchParams.get('endDate') === expected,
		value
	);
}

async function waitForApiResourceCount(page, count) {
	await page.waitForFunction(
		(expected) =>
			performance.getEntriesByType('resource').filter((entry) => entry.name.includes('/api/'))
				.length >= expected,
		count,
		{ timeout: 60_000 }
	);
}

async function activateAllCharts(page) {
	const chartIds = await page
		.locator('[data-chart-id]')
		.evaluateAll((cards) =>
			cards.map((card) => card.getAttribute('data-chart-id')).filter(Boolean)
		);
	for (const chartId of chartIds) {
		const card = page.locator(`[data-chart-id="${chartId}"]`);
		if ((await card.getAttribute('data-chart-activated')) === 'true') continue;
		await page.locator(`[data-chart-sentinel="${chartId}"]`).scrollIntoViewIfNeeded();
		await page.waitForFunction(
			(id) =>
				document.querySelector(`[data-chart-id="${id}"]`)?.getAttribute('data-chart-activated') ===
				'true',
			chartId
		);
	}
}

const browser = await chromium.launch({ headless: true });
try {
	const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
	const client = await page.context().newCDPSession(page);
	await client.send('Network.enable');
	await client.send('Network.emulateNetworkConditions', {
		offline: false,
		latency: latencyMs,
		downloadThroughput: downloadKibPerSecond > 0 ? downloadKibPerSecond * 1024 : -1,
		uploadThroughput: -1,
		connectionType: 'other'
	});

	let stage = 'initial';
	const requests = { initial: [], extend: [], restore: [] };
	const failedResponses = [];
	const requestFailures = [];
	const pageErrors = [];
	page.on('request', (request) => {
		const requestUrl = new URL(request.url());
		if (requestUrl.pathname.startsWith('/api/')) {
			requests[stage].push(requestUrl.pathname);
		}
	});
	page.on('response', (response) => {
		const responseUrl = new URL(response.url());
		if (
			responseUrl.pathname.startsWith('/api/') &&
			(response.status() < 200 || response.status() >= 300)
		) {
			failedResponses.push({ path: responseUrl.pathname, status: response.status() });
		}
	});
	page.on('requestfailed', (request) => {
		const requestUrl = new URL(request.url());
		if (requestUrl.pathname.startsWith('/api/')) {
			requestFailures.push({
				path: requestUrl.pathname,
				error: request.failure()?.errorText ?? 'unknown request failure'
			});
		}
	});
	page.on('pageerror', (error) => pageErrors.push(error.message));

	await page.goto(url.toString(), { waitUntil: 'domcontentloaded', timeout: 60_000 });
	await activateAllCharts(page);
	await waitForApiResourceCount(page, 6);
	await waitForDashboard(page);

	stage = 'extend';
	let startedAt = performance.now();
	await setEndDate(page, '2025-02-03');
	await waitForApiResourceCount(page, 12);
	await waitForDashboard(page);
	const extendMs = Math.round(performance.now() - startedAt);

	stage = 'restore';
	startedAt = performance.now();
	await setEndDate(page, '2025-02-02');
	await waitForDashboard(page);
	const restoreMs = Math.round(performance.now() - startedAt);
	await page.waitForTimeout(latencyMs * 2 + 100);

	console.log(
		JSON.stringify(
			{
				latencyMs,
				downloadKibPerSecond: downloadKibPerSecond || null,
				extendMs,
				restoreMs,
				requests,
				failedResponses,
				requestFailures,
				pageErrors,
				requestCounts: Object.fromEntries(
					Object.entries(requests).map(([key, paths]) => [key, paths.length])
				)
			},
			null,
			2
		)
	);
	if (requests.restore.length > 0) {
		console.error(`Restoring the cached window issued ${requests.restore.length} API requests`);
		process.exitCode = 1;
	}
	if (failedResponses.length > 0 || requestFailures.length > 0 || pageErrors.length > 0) {
		console.error('Dashboard emitted API or page errors; refusing to record a passing profile');
		process.exitCode = 1;
	}
} finally {
	await browser.close();
}
