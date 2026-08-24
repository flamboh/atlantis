import { chromium } from '@playwright/test';

const baseUrl = process.env.ATLANTIS_PERF_BASE_URL ?? 'http://127.0.0.1:4173';
const latencyMs = Number(process.env.ATLANTIS_PERF_LATENCY_MS ?? 80);
const downloadKibPerSecond = Number(process.env.ATLANTIS_PERF_DOWNLOAD_KIB_PER_SECOND ?? 0);
const maximumVisibleReadyMs = Number(process.env.ATLANTIS_PERF_MAX_READY_MS ?? 3_000);
const apiPathByChartId = {
	dashboard: '/api/netflow/stats',
	characteristics: '/api/netflow/characteristics',
	ip: '/api/ip/stats',
	protocol: '/api/protocol/stats',
	spectrum: '/api/netflow/spectrum-stats',
	coverage: '/api/netflow/coverage'
};
const expectedApiPaths = Object.values(apiPathByChartId);
const loadingCopy = [
	'Loading data',
	'Loading flow characteristics',
	'Loading IP data',
	'Loading protocol data',
	'Loading spectrum data',
	'Loading coverage'
];
const url = new URL('/datasets/performance', baseUrl);
url.search = new URLSearchParams({
	startDate: process.env.ATLANTIS_PERF_START_DATE ?? '2025-02-01',
	endDate: process.env.ATLANTIS_PERF_END_DATE ?? '2025-02-15',
	groupBy: 'hour'
}).toString();

async function waitForActivatedCharts(page) {
	await page.waitForFunction(
		(copy) => {
			const activeCards = [...document.querySelectorAll('[data-chart-activated="true"]')];
			return (
				activeCards.length > 0 &&
				activeCards.every((card) => copy.every((text) => !card.textContent?.includes(text)))
			);
		},
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

async function getActivatedChartIds(page) {
	return page
		.locator('[data-chart-activated="true"]')
		.evaluateAll((cards) =>
			cards.map((card) => card.getAttribute('data-chart-id')).filter(Boolean)
		);
}

async function waitForApiResources(page, paths) {
	await page.waitForFunction(
		(expectedPaths) => {
			const completedPaths = new Set(
				performance
					.getEntriesByType('resource')
					.filter((entry) => entry.name.includes('/api/'))
					.map((entry) => new URL(entry.name).pathname)
			);
			return expectedPaths.every((path) => completedPaths.has(path));
		},
		paths,
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
			chartId,
			{ timeout: 60_000 }
		);
	}
}

async function captureMetrics(page, client) {
	await client.send('HeapProfiler.collectGarbage');
	const browserMetrics = await page.evaluate(() => {
		const navigation = performance.getEntriesByType('navigation')[0];
		const resources = performance
			.getEntriesByType('resource')
			.filter((entry) => entry.name.includes('/api/'));
		return {
			navigation: navigation
				? {
						domContentLoadedEventEnd: navigation.domContentLoadedEventEnd,
						responseEnd: navigation.responseEnd,
						transferSize: navigation.transferSize
					}
				: null,
			apiTransferBytes: resources.reduce((total, entry) => total + entry.transferSize, 0),
			apiDecodedBytes: resources.reduce((total, entry) => total + entry.decodedBodySize, 0),
			apiResources: resources.map((entry) => ({
				path: new URL(entry.name).pathname,
				durationMs: Math.round(entry.duration),
				transferBytes: entry.transferSize,
				decodedBytes: entry.decodedBodySize
			})),
			canvasCount: document.querySelectorAll('canvas').length
		};
	});
	const performanceMetrics = Object.fromEntries(
		(await client.send('Performance.getMetrics')).metrics
			.filter(({ name }) =>
				['ScriptDuration', 'LayoutDuration', 'TaskDuration', 'JSHeapUsedSize'].includes(name)
			)
			.map(({ name, value }) => [name, value])
	);
	return { ...browserMetrics, performanceMetrics };
}

const browser = await chromium.launch({ headless: true });
try {
	const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
	const client = await page.context().newCDPSession(page);
	await client.send('Network.enable');
	await client.send('Performance.enable');
	await client.send('HeapProfiler.enable');
	await client.send('Network.emulateNetworkConditions', {
		offline: false,
		latency: latencyMs,
		downloadThroughput: downloadKibPerSecond > 0 ? downloadKibPerSecond * 1024 : -1,
		uploadThroughput: -1,
		connectionType: 'other'
	});

	const apiResponses = [];
	const requestFailures = [];
	const pageErrors = [];
	page.on('response', (response) => {
		const responseUrl = new URL(response.url());
		if (!responseUrl.pathname.startsWith('/api/')) return;
		const headers = response.headers();
		apiResponses.push({
			path: responseUrl.pathname,
			status: response.status(),
			contentLength: Number(headers['content-length'] ?? 0)
		});
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

	const startedAt = performance.now();
	await page.goto(url.toString(), { waitUntil: 'domcontentloaded', timeout: 60_000 });
	const domContentLoadedMs = performance.now() - startedAt;
	await page.waitForFunction(
		() => document.querySelectorAll('[data-chart-activated="true"]').length > 0
	);
	const initiallyActivatedCharts = await getActivatedChartIds(page);
	const initialExpectedPaths = initiallyActivatedCharts.map((chartId) => apiPathByChartId[chartId]);
	await waitForApiResources(page, initialExpectedPaths);
	await waitForActivatedCharts(page);
	const visibleReadyMs = performance.now() - startedAt;
	const initialResponseCount = apiResponses.length;
	const visibleMetrics = await captureMetrics(page, client);

	const activationStartedAt = performance.now();
	await activateAllCharts(page);
	await waitForApiResources(page, expectedApiPaths);
	await waitForActivatedCharts(page);
	const activateAllChartsMs = performance.now() - activationStartedAt;
	const allChartsReadyMs = performance.now() - startedAt;
	const allChartsMetrics = await captureMetrics(page, client);

	const initialApiResponses = apiResponses.slice(0, initialResponseCount);
	const result = {
		url: url.toString(),
		latencyMs,
		downloadKibPerSecond: downloadKibPerSecond || null,
		domContentLoadedMs: Math.round(domContentLoadedMs),
		visibleReadyMs: Math.round(visibleReadyMs),
		maximumVisibleReadyMs,
		activateAllChartsMs: Math.round(activateAllChartsMs),
		allChartsReadyMs: Math.round(allChartsReadyMs),
		initiallyActivatedCharts,
		initialApiResponses,
		apiResponses,
		requestFailures,
		pageErrors,
		visibleMetrics,
		allChartsMetrics
	};
	console.log(JSON.stringify(result, null, 2));

	const failedResponses = apiResponses.filter(({ status }) => status < 200 || status >= 300);
	const successfulPaths = new Set(
		apiResponses.filter(({ status }) => status >= 200 && status < 300).map(({ path }) => path)
	);
	const initialSuccessfulPaths = new Set(
		initialApiResponses
			.filter(({ status }) => status >= 200 && status < 300)
			.map(({ path }) => path)
	);
	const missingInitialPaths = initialExpectedPaths.filter(
		(path) => !initialSuccessfulPaths.has(path)
	);
	const unexpectedInitialPaths = expectedApiPaths.filter(
		(path) =>
			!initialExpectedPaths.includes(path) && initialApiResponses.some((item) => item.path === path)
	);
	const missingPaths = expectedApiPaths.filter((path) => !successfulPaths.has(path));

	if (failedResponses.length > 0 || requestFailures.length > 0 || pageErrors.length > 0) {
		console.error('Dashboard emitted API or page errors; refusing to record a passing profile');
		process.exitCode = 1;
	}
	if (initiallyActivatedCharts.length !== 1) {
		console.error(
			`Expected one initially activated chart, found ${initiallyActivatedCharts.length}`
		);
		process.exitCode = 1;
	}
	if (missingInitialPaths.length > 0 || unexpectedInitialPaths.length > 0) {
		console.error(
			`Initial requests did not match visible cards; missing: ${missingInitialPaths.join(', ') || 'none'}, unexpected: ${unexpectedInitialPaths.join(', ') || 'none'}`
		);
		process.exitCode = 1;
	}
	if (missingPaths.length > 0) {
		console.error(`Dashboard did not complete successful requests for: ${missingPaths.join(', ')}`);
		process.exitCode = 1;
	}
	if (visibleReadyMs > maximumVisibleReadyMs) {
		console.error(
			`Visible dashboard took ${Math.round(visibleReadyMs)} ms to become interactive; budget is ${maximumVisibleReadyMs} ms`
		);
		process.exitCode = 1;
	}
} finally {
	await browser.close();
}
