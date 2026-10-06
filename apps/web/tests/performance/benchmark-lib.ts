import { readFile, readdir } from 'node:fs/promises';
import { join, relative, resolve } from 'node:path';
import { gzipSync } from 'node:zlib';

export function distribution(samples: (number | null | undefined)[]) {
	const values = samples
		.filter((value): value is number => typeof value === 'number' && Number.isFinite(value))
		.sort((a, b) => a - b);
	if (values.length === 0) return { count: 0, p50: null, p95: null, min: null, max: null };
	const percentile = (p: number) => values[Math.max(0, Math.ceil(values.length * p) - 1)];
	return {
		count: values.length,
		p50: percentile(0.5),
		p95: percentile(0.95),
		min: values[0],
		max: values.at(-1)
	};
}

export function pacificEpoch(date: string, end = false) {
	if (!/^\d{4}-\d{2}-\d{2}$/.test(date) || !Number.isFinite(Date.parse(date)))
		throw new Error(`Invalid date: ${date}`);
	const day = new Date(`${date}T00:00:00Z`);
	if (day.toISOString().slice(0, 10) !== date) throw new Error(`Invalid date: ${date}`);
	if (end) day.setUTCDate(day.getUTCDate() + 1);
	const target = day.getTime();
	const formatter = new Intl.DateTimeFormat('en-US', {
		timeZone: 'America/Los_Angeles',
		year: 'numeric',
		month: '2-digit',
		day: '2-digit',
		hour: '2-digit',
		minute: '2-digit',
		second: '2-digit',
		hourCycle: 'h23'
	});
	let guess = target + 8 * 3_600_000;
	for (let i = 0; i < 3; i++) {
		const parts = Object.fromEntries(
			formatter.formatToParts(guess).map(({ type, value }) => [type, value])
		);
		const wallClock = Date.UTC(
			Number(parts.year),
			Number(parts.month) - 1,
			Number(parts.day),
			Number(parts.hour),
			Number(parts.minute),
			Number(parts.second)
		);
		guess += target - wallClock;
	}
	return guess / 1000;
}

export function queryScenarios(
	dataset: { datasetId: string; hasLocality?: boolean },
	routers: string[],
	startDate: string,
	endDate: string
) {
	if (!routers.length) throw new Error('The dataset has no sources');
	if (pacificEpoch(startDate) >= pacificEpoch(endDate, true))
		throw new Error('Start date must be on or before end date');
	const dateAfter = (days: number) =>
		new Date(Date.parse(`${startDate}T00:00:00Z`) + days * 86_400_000).toISOString().slice(0, 10);
	const make = (
		name: string,
		end: string,
		groupBy: string,
		selectedRouters: string[],
		direction = 'all',
		ipVersion = '4'
	) => ({
		name,
		startDate,
		endDate: end,
		groupBy,
		params: {
			dataset: dataset.datasetId,
			routers: selectedRouters.join(','),
			startDate: String(pacificEpoch(startDate)),
			endDate: String(pacificEpoch(end, true)),
			groupBy,
			granularity: groupBy === 'date' ? '1d' : '1h',
			direction,
			ipVersion,
			measure: 'addresses'
		}
	});
	return [
		make('default', endDate, 'date', routers),
		make('long-range', [endDate, dateAfter(179)].sort()[0], 'date', routers),
		make(
			'filtered-hourly',
			[endDate, dateAfter(6)].sort()[0],
			'hour',
			routers.slice(0, 1),
			dataset.hasLocality ? 'ingress' : 'all'
		),
		make('ipv6-hourly', [endDate, dateAfter(6)].sort()[0], 'hour', routers.slice(0, 1), 'all', '6')
	];
}

type BundleFile = { file: string; type: string; rawBytes: number; gzipBytes: number };
type ManifestChunk = { name: string; file: string; css?: string[]; imports?: string[] };

export async function measureBundles(directory: string) {
	const manifest: Record<string, ManifestChunk> = JSON.parse(
		await readFile(join(directory, '.vite/manifest.json'), 'utf8')
	);
	const files: BundleFile[] = [];
	async function walk(path: string) {
		for (const entry of await readdir(path, { withFileTypes: true })) {
			const full = join(path, entry.name);
			if (entry.isDirectory()) await walk(full);
			else if (/\.(js|css)$/.test(entry.name)) {
				const contents = await readFile(full);
				files.push({
					file: relative(directory, full),
					type: entry.name.endsWith('.css') ? 'css' : 'js',
					rawBytes: contents.length,
					gzipBytes: gzipSync(contents, { level: 9 }).length
				});
			}
		}
	}
	await walk(directory);
	const sum = (selected: BundleFile[]) => ({
		rawBytes: selected.reduce((sum, file) => sum + file.rawBytes, 0),
		gzipBytes: selected.reduce((sum, file) => sum + file.gzipBytes, 0)
	});
	function closure(keys: string[]) {
		const seen = new Set();
		const names = new Set();
		function visit(key: string) {
			if (seen.has(key)) return;
			seen.add(key);
			const chunk = manifest[key];
			if (!chunk) throw new Error(`Manifest dependency is missing: ${key}`);
			names.add(chunk.file);
			for (const css of chunk.css ?? []) names.add(css);
			for (const dependency of chunk.imports ?? []) visit(dependency);
		}
		keys.forEach(visit);
		return files.filter(({ file }) => names.has(file));
	}
	const boot = Object.keys(manifest).filter(
		(key) => /^entry\//.test(manifest[key].name) || manifest[key].name === 'client-entry'
	);
	const nodes: { key: string; route: string; kind: string }[] = [];
	for (const [key, chunk] of Object.entries(manifest)) {
		if (!/^nodes\//.test(chunk.name)) continue;
		const source = await readFile(
			join(resolve(directory, '../../generated/build/client-optimized'), `${chunk.name}.js`),
			'utf8'
		);
		const component = /src\/routes\/(.*?)\+(page|layout)\.svelte/.exec(source);
		if (component)
			nodes.push({ key, route: `/${component[1]}`.replace(/\/$/, '') || '/', kind: component[2] });
	}
	const routes = nodes
		.filter(({ kind }) => kind === 'page')
		.map(({ key, route }) => {
			const layouts = nodes
				.filter(
					(node) =>
						node.kind === 'layout' &&
						(node.route === '/' || route === node.route || route.startsWith(`${node.route}/`))
				)
				.map(({ key }) => key);
			const selected = closure([...boot, ...layouts, key]);
			return {
				route,
				entry: manifest[key].file,
				...sum(selected),
				js: sum(selected.filter(({ type }) => type === 'js')),
				css: sum(selected.filter(({ type }) => type === 'css')),
				files: selected.map(({ file }) => file)
			};
		});
	if (!routes.length) throw new Error('No route entries found in the production manifest');
	return {
		total: sum(files),
		js: sum(files.filter(({ type }) => type === 'js')),
		css: sum(files.filter(({ type }) => type === 'css')),
		files: files.sort((a, b) => b.rawBytes - a.rawBytes),
		routes
	};
}

type Distribution = ReturnType<typeof distribution>;
type SummaryReport = {
	createdAt: string;
	baseUrl: string;
	environment: { commit: string };
	dataset: { datasetId: string };
	scenarios: ReturnType<typeof queryScenarios>;
	options: { warmups: number; runs: number; pageRuns: number; cpuSlowdown: number };
	bundles: Awaited<ReturnType<typeof measureBundles>>;
	api: {
		scenario: string;
		route: string;
		latencyMs: Distribution;
		samples: { decodedBytes: number }[];
		error: string | null;
	}[];
	pages: { name: string; summary: Record<string, Distribution> }[];
	errors: string[];
};

export function markdownSummary(report: SummaryReport) {
	const ms = (value: number | null | undefined) =>
		value === null || value === undefined ? 'n/a' : value.toFixed(1);
	const kib = (value: number) => (value / 1024).toFixed(1);
	return [
		'# Dashboard performance baseline',
		'',
		`Captured ${report.createdAt} at ${report.baseUrl}. Commit ${report.environment.commit}.`,
		'',
		`Dataset ${report.dataset.datasetId}; ${report.scenarios[0].startDate} through ${report.scenarios[0].endDate}, inclusive Pacific dates. Sequential API requests, ${report.options.warmups} discarded warmups and ${report.options.runs} measured runs per query. Fresh browser contexts, ${report.options.pageRuns} runs per page, viewport 1280 x 800, CPU slowdown ${report.options.cpuSlowdown}x.`,
		'',
		'## API latency',
		'',
		'| Scenario | Route | p50 ms | p95 ms | Body KiB | Error |',
		'| --- | --- | ---: | ---: | ---: | --- |',
		...report.api.map(
			(item) =>
				`| ${item.scenario} | ${item.route} | ${ms(item.latencyMs.p50)} | ${ms(item.latencyMs.p95)} | ${item.samples.length ? kib(item.samples[0].decodedBytes) : 'n/a'} | ${item.error ?? ''} |`
		),
		'',
		'## Production bundles',
		'',
		`All JS: ${kib(report.bundles.js.rawBytes)} KiB raw / ${kib(report.bundles.js.gzipBytes)} KiB gzip. All CSS: ${kib(report.bundles.css.rawBytes)} KiB raw / ${kib(report.bundles.css.gzipBytes)} KiB gzip. Gzip level 9, each file compressed separately.`,
		'',
		'Route totals include static imports, root layout, and boot entries, deduplicated within each route. Shared chunks count once per route; route totals must not be added together. Dynamically loaded chunks appear in the file inventory and browser resources.',
		'',
		'| Route | Entry chunk | Raw KiB | Gzip KiB |',
		'| --- | --- | ---: | ---: |',
		...report.bundles.routes.map(
			(route) =>
				`| ${route.route} | ${route.entry} | ${kib(route.rawBytes)} | ${kib(route.gzipBytes)} |`
		),
		'',
		'| Largest file | Raw KiB | Gzip KiB |',
		'| --- | ---: | ---: |',
		...report.bundles.files
			.slice(0, 10)
			.map((file) => `| ${file.file} | ${kib(file.rawBytes)} | ${kib(file.gzipBytes)} |`),
		'',
		'## Browser loads',
		'',
		'| Page | TTFB p50 | FCP p50 | LCP p50 | Visible charts p50 | All charts p50 | Initial TBT p50 | All-chart heap MiB p50 |',
		'| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |',
		...report.pages.map(
			(page) =>
				`| ${page.name} | ${ms(page.summary.ttfbMs.p50)} | ${ms(page.summary.fcpMs.p50)} | ${ms(page.summary.lcpMs.p50)} | ${ms(page.summary.visibleChartsMs.p50)} | ${ms(page.summary.allChartsMs.p50)} | ${ms(page.summary.tbtMs.p50)} | ${ms(page.summary.heapUsedBytes.p50 === null ? null : page.summary.heapUsedBytes.p50 / 1024 ** 2)} |`
		),
		'',
		'LCP is the last observed candidate before scrolling. Initial TBT sums the portion over 50 ms of long tasks between FCP and visible-chart readiness plus the settling interval. It is an observation-window metric, not Lighthouse TBT. JSON also contains post-scroll long tasks, p95 values, per-card readiness, API/resource timings, and heap before/after explicit GC. Charts are ready when their loading text disappears, a chart graphic exists, and two animation frames pass. This excludes deferred cards from initial readiness.',
		'',
		`Status: ${report.errors.length ? 'FAILED' : 'passed'}. ${report.errors.join('; ')}`,
		''
	].join('\n');
}
