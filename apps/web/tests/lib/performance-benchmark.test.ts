import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import {
	distribution,
	measureBundles,
	pacificEpoch,
	queryScenarios
} from '../performance/benchmark-lib.ts';

describe('performance benchmark measurements', () => {
	it('uses nearest-rank percentiles and excludes missing browser metrics', () => {
		expect(distribution([50, 10, null, 30, 20, 40])).toEqual({
			count: 5,
			p50: 30,
			p95: 50,
			min: 10,
			max: 50
		});
		expect(distribution([null])).toEqual({ count: 0, p50: null, p95: null, min: null, max: null });
	});
	it('matches Pacific inclusive UI dates through both DST transitions', () => {
		expect(pacificEpoch('2025-06-01')).toBe(Date.parse('2025-06-01T07:00:00Z') / 1000);
		expect(pacificEpoch('2025-03-09', true) - pacificEpoch('2025-03-09')).toBe(23 * 3600);
		expect(pacificEpoch('2025-11-02', true) - pacificEpoch('2025-11-02')).toBe(25 * 3600);
		expect(() => pacificEpoch('2025-02-30')).toThrow();
	});
	it('uses bounded grouped queries, source filtering, and valid locality settings', () => {
		const scenarios = queryScenarios(
			{ datasetId: 'sample', hasLocality: false },
			['a', 'b'],
			'2025-06-01',
			'2026-06-30'
		);
		expect(scenarios[0].params.routers).toBe('a,b');
		expect(scenarios[1].endDate).toBe('2025-11-27');
		expect(scenarios[2]).toMatchObject({
			endDate: '2025-06-07',
			params: { routers: 'a', direction: 'all', groupBy: 'hour', granularity: '1h' }
		});
		expect(scenarios[3].params.ipVersion).toBe('6');
		expect(
			queryScenarios(
				{ datasetId: 'sample', hasLocality: true },
				['a'],
				'2025-06-01',
				'2026-06-30'
			)[2].params.direction
		).toBe('ingress');
		expect(() => queryScenarios({ datasetId: 'sample' }, [], '2025-06-01', '2026-06-30')).toThrow();
	});
});

describe('bundle route accounting', () => {
	let directory: string;
	afterEach(async () => {
		if (directory) await rm(directory, { recursive: true, force: true });
	});
	it('deduplicates shared imports per route and includes boot, layouts, and CSS', async () => {
		directory = await mkdtemp(join(tmpdir(), 'atlantis-bundles-'));
		const client = join(directory, 'output/client');
		const generated = join(directory, 'generated/build/client-optimized/nodes');
		await mkdir(join(client, '.vite'), { recursive: true });
		await mkdir(generated, { recursive: true });
		const manifest = {
			boot: { name: 'entry/start', file: 'boot.js', imports: ['shared'] },
			shared: { name: 'shared', file: 'shared.js' },
			layout: { name: 'nodes/0', file: 'layout.js', imports: ['shared'], css: ['layout.css'] },
			page: { name: 'nodes/2', file: 'page.js', imports: ['shared'] }
		};
		await writeFile(join(client, '.vite/manifest.json'), JSON.stringify(manifest));
		await writeFile(
			join(generated, '0.js'),
			'export { default as component } from "../../src/routes/+layout.svelte";'
		);
		await writeFile(
			join(generated, '2.js'),
			'export { default as component } from "../../src/routes/example/+page.svelte";'
		);
		for (const file of ['boot.js', 'shared.js', 'layout.js', 'layout.css', 'page.js'])
			await writeFile(join(client, file), 'x'.repeat(100));
		const result = await measureBundles(client);
		expect(result.total.rawBytes).toBe(500);
		expect(result.routes).toHaveLength(1);
		expect(result.routes[0]).toMatchObject({
			route: '/example',
			rawBytes: 500,
			js: { rawBytes: 400 },
			css: { rawBytes: 100 }
		});
		expect(result.routes[0].files).toHaveLength(5);
		expect(result.total.gzipBytes).toBeLessThan(result.total.rawBytes);
	});
});
