import { chromium } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { cpus, hostname, platform, release } from 'node:os';
import { dirname, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { distribution, markdownSummary, measureBundles, queryScenarios } from './benchmark-lib.ts';

const { values } = parseArgs({
	options: {
		'base-url': {
			type: 'string',
			default: process.env.ATLANTIS_PERF_BASE_URL ?? 'http://127.0.0.1:8090'
		},
		dataset: { type: 'string' },
		'start-date': { type: 'string' },
		'end-date': { type: 'string' },
		'file-slug': { type: 'string' },
		output: { type: 'string', default: '/tmp/atlantis-perf/baseline' },
		'bundle-dir': { type: 'string', default: 'apps/web/.svelte-kit/output/client' },
		runs: { type: 'string', default: '5' },
		'page-runs': { type: 'string', default: '3' },
		warmups: { type: 'string', default: '1' },
		'timeout-ms': { type: 'string', default: '120000' },
		'settle-ms': { type: 'string', default: '1000' },
		'cpu-slowdown': { type: 'string', default: '1' },
		'chart-selector': {
			type: 'string',
			default: 'canvas, svg[role="img"], [data-chart-rendered="true"]'
		},
		help: { type: 'boolean', default: false }
	}
});

if (values.help) {
	console.log(
		'bun run perf --base-url URL [--dataset ID] [--start-date YYYY-MM-DD] [--end-date YYYY-MM-DD] [--output /tmp/prefix] [--runs 5] [--page-runs 3] [--warmups 1] [--bundle-dir apps/web/.svelte-kit/output/client] [--cpu-slowdown 1] [--chart-selector CSS]'
	);
	process.exit(0);
}
const options = Object.fromEntries(
	['runs', 'page-runs', 'warmups', 'timeout-ms', 'settle-ms', 'cpu-slowdown'].map((key) => [
		key.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase()),
		Number(values[key])
	])
);
for (const [key, value] of Object.entries(options)) {
	if (!Number.isInteger(value) || value < (key === 'warmups' || key === 'settleMs' ? 0 : 1))
		throw new Error(`Invalid ${key}`);
}
const baseUrl = new URL(values['base-url']).origin;
async function fetchJson(path) {
	const started = performance.now();
	const response = await fetch(new URL(path, baseUrl), {
		signal: AbortSignal.timeout(options.timeoutMs)
	});
	const body = await response.arrayBuffer();
	const latencyMs = performance.now() - started;
	if (!response.ok) throw new Error(`${path}: HTTP ${response.status}`);
	const payload = JSON.parse(new TextDecoder().decode(body));
	if (payload.error) throw new Error(`${path}: ${payload.error}`);
	return { payload, latencyMs, decodedBytes: body.byteLength, status: response.status };
}
const datasets = (await fetchJson('/api/datasets')).payload.data;
const dataset = values.dataset
	? datasets?.find((item) => item.datasetId === values.dataset)
	: (datasets?.find((item) => item.isDefault) ?? datasets?.[0]);
if (!dataset) throw new Error('Requested dataset was not returned by /api/datasets');
const routers = (await fetchJson(`/api/routers?dataset=${encodeURIComponent(dataset.datasetId)}`))
	.payload;
const maad = (
	await fetchJson(`/api/netflow/maad-status?dataset=${encodeURIComponent(dataset.datasetId)}`)
).payload;
const scenarios = queryScenarios(
	dataset,
	routers,
	values['start-date'] ?? dataset.defaultStartDate,
	values['end-date'] ?? new Date().toISOString().slice(0, 10)
);
const report = {
	schemaVersion: 1,
	createdAt: new Date().toISOString(),
	baseUrl,
	dataset,
	routers,
	maad,
	options,
	scenarios,
	environment: {
		commit: execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(),
		dirty: Boolean(execFileSync('git', ['status', '--porcelain'], { encoding: 'utf8' }).trim()),
		node: process.version,
		hostname: hostname(),
		platform: platform(),
		kernel: release(),
		cpu: cpus()[0]?.model,
		logicalCpus: cpus().length,
		browser: null,
		network: 'unthrottled loopback or supplied base URL; server and OS caches are warm'
	},
	bundles: await measureBundles(resolve(values['bundle-dir'])),
	api: [],
	pages: [],
	errors: []
};
const routes = [
	'/api/netflow/stats',
	'/api/netflow/characteristics',
	'/api/ip/stats',
	'/api/protocol/stats',
	'/api/netflow/coverage',
	...(maad.computed
		? [
				'/api/netflow/dimension-stats',
				'/api/netflow/spectrum-stats',
				'/api/netflow/structure-stats'
			]
		: [])
];
for (const scenario of [
	{
		name: 'metadata',
		params: { dataset: dataset.datasetId },
		routes: ['/api/datasets', '/api/routers', '/api/netflow/maad-status']
	},
	...scenarios
]) {
	for (const route of scenario.routes ?? routes) {
		console.log(`API ${scenario.name} ${route}`);
		const path = `${route}?${new URLSearchParams(scenario.params)}`;
		const samples = [];
		let error = null;
		try {
			for (let run = -options.warmups; run < options.runs; run++) {
				const { payload, ...sample } = await fetchJson(path);
				if (
					scenario.name !== 'metadata' &&
					!payload.result &&
					!payload.timelines &&
					!payload.observationBuckets
				)
					throw new Error('Unexpected aggregate response shape');
				if (run >= 0) samples.push(sample);
			}
		} catch (reason) {
			error = String(reason);
			report.errors.push(error);
		}
		report.api.push({
			scenario: scenario.name,
			route,
			params: scenario.params,
			samples,
			latencyMs: distribution(samples.map(({ latencyMs }) => latencyMs)),
			error
		});
	}
}

const loadingCopy = [
	'Loading data',
	'Loading flow characteristics',
	'Loading IP data',
	'Loading protocol data',
	'Loading spectrum data',
	'Loading coverage',
	'Loading dimension data',
	'Loading port'
];
async function waitForCharts(page, ids) {
	await page.waitForFunction(
		({ ids, selector, loadingCopy }) =>
			ids.every((id) => {
				const card = document.querySelector(`[data-chart-id="${id}"]`);
				return (
					card?.getAttribute('data-chart-activated') === 'true' &&
					loadingCopy.every((copy) => !card.textContent.includes(copy)) &&
					(card.querySelector(selector) ||
						card.querySelector(
							'[data-testid="chart-unavailable"], [data-testid="chart-selection-unavailable"]'
						))
				);
			}),
		{ ids, selector: values['chart-selector'], loadingCopy },
		{ timeout: options.timeoutMs }
	);
	return page.evaluate(async () => {
		await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
		return performance.now();
	});
}
async function capture(page, client, cutoff, initial) {
	const metrics = await page.evaluate(
		({ cutoff, initial }) => {
			const navigation = performance.getEntriesByType('navigation')[0];
			const fcp = performance.getEntriesByName('first-contentful-paint')[0]?.startTime ?? null;
			const tasks = window.__atlantisPerf.tasks.filter((task) => task.startTime < cutoff);
			const initialTasks = tasks.filter(
				(task) => fcp !== null && task.startTime + task.duration > fcp
			);
			return {
				ttfbMs: navigation.responseStart - navigation.startTime,
				fcpMs: fcp,
				lcpMs: window.__atlantisPerf.lcp,
				domContentLoadedMs: navigation.domContentLoadedEventEnd,
				tbtMs: initialTasks.reduce(
					(sum, task) =>
						sum +
						Math.max(
							0,
							Math.min(task.startTime + task.duration, cutoff) - Math.max(task.startTime, fcp) - 50
						),
					0
				),
				longTaskCount: tasks.length,
				longTaskMs: tasks.reduce((sum, task) => sum + task.duration, 0),
				longTasks: tasks,
				resources: performance.getEntriesByType('resource').map((entry) => ({
					path: new URL(entry.name).pathname,
					durationMs: entry.duration,
					transferBytes: entry.transferSize,
					decodedBytes: entry.decodedBodySize,
					initiatorType: entry.initiatorType
				})),
				canvasCount: document.querySelectorAll('canvas').length,
				observedUntilMs: cutoff,
				phase: initial ? 'initial' : 'all-charts'
			};
		},
		{ cutoff, initial }
	);
	const metricsByName = async () =>
		Object.fromEntries(
			(await client.send('Performance.getMetrics')).metrics.map(({ name, value }) => [name, value])
		);
	const before = await metricsByName();
	await client.send('HeapProfiler.collectGarbage');
	const after = await metricsByName();
	return {
		...metrics,
		heapUsedBytes: before.JSHeapUsedSize,
		heapAfterGcBytes: after.JSHeapUsedSize,
		scriptDurationMs: before.ScriptDuration * 1000,
		layoutDurationMs: before.LayoutDuration * 1000,
		taskDurationMs: before.TaskDuration * 1000
	};
}

const dashboardUrl = (scenario) =>
	`/datasets/${encodeURIComponent(dataset.datasetId)}?${new URLSearchParams({ startDate: scenario.startDate, endDate: scenario.endDate, groupBy: scenario.groupBy, direction: scenario.params.direction, ipVersion: scenario.params.ipVersion })}`;
const pageCases = [
	{ name: 'datasets', path: '/', charted: false, ready: 'main h1' },
	{ name: 'dashboard-default', path: dashboardUrl(scenarios[0]), charted: true },
	{ name: 'dashboard-long-range', path: dashboardUrl(scenarios[1]), charted: true },
	{ name: 'dashboard-filtered-hourly', path: dashboardUrl(scenarios[2]), charted: true },
	{
		name: 'files',
		path: `/netflow/files?dataset=${encodeURIComponent(dataset.datasetId)}`,
		charted: false,
		ready: '#timestamp'
	}
];
if (maad.computed)
	pageCases.push({
		name: 'file-detail',
		path: `/netflow/files/${values['file-slug'] ?? `${scenarios[0].startDate.replaceAll('-', '')}0000`}?dataset=${encodeURIComponent(dataset.datasetId)}`,
		charted: false,
		fileDetail: true
	});
let browser;
try {
	browser = await chromium.launch({ headless: true });
	report.environment.browser = browser.version();
	for (const pageCase of pageCases) {
		const samples = [];
		for (let run = 0; run < options.pageRuns; run++) {
			console.log(`PAGE ${pageCase.name} ${run + 1}/${options.pageRuns}`);
			const context = await browser.newContext({
				viewport: { width: 1280, height: 800 },
				timezoneId: 'America/Los_Angeles',
				reducedMotion: 'reduce'
			});
			try {
				const page = await context.newPage();
				const errors = [];
				page.on('pageerror', (error) => errors.push(error.message));
				page.on('response', (response) => {
					if (response.status() >= 400)
						errors.push(`HTTP ${response.status()} ${new URL(response.url()).pathname}`);
				});
				page.on('requestfailed', (request) => {
					if (request.failure()?.errorText !== 'net::ERR_ABORTED')
						errors.push(`${new URL(request.url()).pathname}: ${request.failure()?.errorText}`);
				});
				await context.addInitScript(() => {
					window.__atlantisPerf = { lcp: null, tasks: [] };
					new PerformanceObserver((list) => {
						window.__atlantisPerf.lcp = list.getEntries().at(-1)?.startTime ?? null;
					}).observe({ type: 'largest-contentful-paint', buffered: true });
					new PerformanceObserver((list) => {
						window.__atlantisPerf.tasks.push(
							...list.getEntries().map(({ startTime, duration }) => ({ startTime, duration }))
						);
					}).observe({ type: 'longtask', buffered: true });
				});
				const client = await context.newCDPSession(page);
				await client.send('Performance.enable');
				await client.send('Emulation.setCPUThrottlingRate', { rate: options.cpuSlowdown });
				const navigation = await page.goto(new URL(pageCase.path, baseUrl).href, {
					waitUntil: 'load',
					timeout: options.timeoutMs
				});
				if (!navigation?.ok()) throw new Error(`Page returned ${navigation?.status()}`);
				let visibleChartsMs = null;
				let allChartsMs = null;
				const cardReadiness = [];
				if (pageCase.charted) {
					await page.waitForSelector('[data-chart-activated="true"]', {
						timeout: options.timeoutMs
					});
					const initialIds = await page
						.locator('[data-chart-activated="true"]')
						.evaluateAll((cards) => cards.map((card) => card.getAttribute('data-chart-id')));
					visibleChartsMs = await waitForCharts(page, initialIds);
				} else if (pageCase.fileDetail) {
					await page.waitForFunction(
						(selector) =>
							document.querySelector(selector) &&
							!/Loading|Refreshing/.test(document.body.textContent),
						values['chart-selector'],
						{ timeout: options.timeoutMs }
					);
					visibleChartsMs = await page.evaluate(async () => {
						await new Promise((resolve) =>
							requestAnimationFrame(() => requestAnimationFrame(resolve))
						);
						return performance.now();
					});
					allChartsMs = visibleChartsMs;
				} else await page.locator(pageCase.ready).first().waitFor();
				await page.waitForTimeout(options.settleMs);
				const initial = await capture(
					page,
					client,
					await page.evaluate(() => performance.now()),
					true
				);
				if (pageCase.charted) {
					const ids = await page
						.locator('[data-chart-id]')
						.evaluateAll((cards) => cards.map((card) => card.getAttribute('data-chart-id')));
					for (const id of ids) {
						const card = page.locator(`[data-chart-id="${id}"]`);
						if ((await card.getAttribute('data-chart-activated')) !== 'true')
							await page.locator(`[data-chart-sentinel="${id}"]`).scrollIntoViewIfNeeded();
						cardReadiness.push({ id, readyMs: await waitForCharts(page, [id]) });
					}
					allChartsMs = await waitForCharts(page, ids);
					await page.waitForTimeout(options.settleMs);
				}
				const all = pageCase.charted
					? await capture(page, client, await page.evaluate(() => performance.now()), false)
					: initial;
				const localFiles = new Set(report.bundles.files.map(({ file }) => `/${file}`));
				for (const resource of all.resources)
					if (/\.(js|css)$/.test(resource.path) && !localFiles.has(resource.path))
						errors.push(`Served asset is absent from the measured build: ${resource.path}`);
				samples.push({ ...initial, visibleChartsMs, allChartsMs, cardReadiness, all, errors });
				report.errors.push(...errors.map((error) => `${pageCase.name}: ${error}`));
			} catch (error) {
				report.errors.push(`${pageCase.name} run ${run + 1}: ${error}`);
			} finally {
				await context.close();
			}
		}
		const metrics = [
			'ttfbMs',
			'fcpMs',
			'lcpMs',
			'visibleChartsMs',
			'allChartsMs',
			'tbtMs',
			'heapUsedBytes',
			'heapAfterGcBytes',
			'longTaskMs',
			'longTaskCount'
		];
		report.pages.push({
			...pageCase,
			samples,
			summary: Object.fromEntries(
				metrics.map((metric) => [
					metric,
					distribution(
						samples.map((sample) =>
							metric.startsWith('heap') ? sample.all[metric] : sample[metric]
						)
					)
				])
			)
		});
	}
} catch (error) {
	report.errors.push(String(error));
} finally {
	await browser?.close();
}
await mkdir(dirname(resolve(values.output)), { recursive: true });
await writeFile(`${values.output}.json`, `${JSON.stringify(report, null, 2)}\n`);
await writeFile(`${values.output}.md`, markdownSummary(report));
console.log(`Saved ${values.output}.{json,md}; ${report.errors.length} errors`);
if (report.errors.length) process.exitCode = 1;
