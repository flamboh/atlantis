import Database from 'better-sqlite3';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
	cleanupPlaywrightDatabase,
	seedPlaywrightDatabase
} from '../../../e2e/playwright-fixture.js';

type RouteModule = { GET: (event: never) => unknown };

const cloudflareEnv = vi.hoisted(() => ({ DB: null as unknown }));

vi.mock('cloudflare:workers', () => ({ env: cloudflareEnv }));

const fixtures: string[] = [];

function seedDatabase(): string {
	const { databasePath, fixtureDirectory } = seedPlaywrightDatabase();
	fixtures.push(fixtureDirectory);
	return databasePath;
}

function createD1Binding(databasePath: string) {
	const sqlite = new Database(databasePath, { readonly: true });
	const toD1Value = (value: unknown) => (Buffer.isBuffer(value) ? [...value] : value);
	return {
		prepare(sql: string) {
			return {
				bind(...params: unknown[]) {
					return {
						async all() {
							const rows = sqlite.prepare(sql).all(...params) as Record<string, unknown>[];
							return {
								results: rows.map((row) =>
									Object.fromEntries(
										Object.entries(row).map(([key, value]) => [key, toD1Value(value)])
									)
								)
							};
						}
					};
				}
			};
		},
		close: () => sqlite.close()
	};
}

async function loadDriver(driver: 'sqlite' | 'd1', databasePath: string) {
	vi.resetModules();
	if (driver === 'd1') {
		const binding = createD1Binding(databasePath);
		cloudflareEnv.DB = binding;
		vi.doMock('#db', () => import('../../../../src/lib/server/db/d1.ts'));
		return binding;
	}
	vi.stubEnv('LOCAL_SQLITE_PATH', databasePath);
	vi.doMock('#db', () => import('../../../../src/lib/server/db/sqlite.ts'));
	return null;
}

async function callRoute(
	driver: 'sqlite' | 'd1',
	databasePath: string,
	route: () => Promise<RouteModule>,
	url: string,
	params: Record<string, string> = {}
) {
	const binding = await loadDriver(driver, databasePath);
	try {
		const { GET } = await route();
		const response = (await GET({ url: new URL(url), params } as never)) as Response;
		return { status: response.status, body: await response.json() };
	} finally {
		binding?.close();
	}
}

const WINDOW = 'startDate=1740823200&endDate=1740823500&granularity=5m&routers=fixture-router';

const routes: Array<{
	name: string;
	route: () => Promise<RouteModule>;
	url: string;
	params?: Record<string, string>;
}> = [
	{
		name: 'datasets',
		route: () => import('../../../../src/routes/api/datasets/+server.ts'),
		url: 'http://localhost/api/datasets'
	},
	{
		name: 'maad-status',
		route: () => import('../../../../src/routes/api/netflow/maad-status/+server.ts'),
		url: 'http://localhost/api/netflow/maad-status?dataset=playwright'
	},
	{
		name: 'dimension-stats',
		route: () => import('../../../../src/routes/api/netflow/dimension-stats/+server.ts'),
		url: `http://localhost/api/netflow/dimension-stats?dataset=playwright&${WINDOW}&measure=packets`
	},
	{
		name: 'structure-stats',
		route: () => import('../../../../src/routes/api/netflow/structure-stats/+server.ts'),
		url: `http://localhost/api/netflow/structure-stats?dataset=playwright&${WINDOW}`
	},
	{
		name: 'spectrum-stats',
		route: () => import('../../../../src/routes/api/netflow/spectrum-stats/+server.ts'),
		url: `http://localhost/api/netflow/spectrum-stats?dataset=playwright&${WINDOW}`
	},
	{
		name: 'file spectrum',
		route: () => import('../../../../src/routes/api/netflow/files/[slug]/spectrum/+server.ts'),
		url: 'http://localhost/api/netflow/files/202503010200/spectrum?dataset=playwright&router=fixture-router&source=true',
		params: { slug: '202503010200' }
	},
	{
		name: 'file structure',
		route: () => import('../../../../src/routes/api/netflow/files/[slug]/structure/+server.ts'),
		url: 'http://localhost/api/netflow/files/202503010200/structure?dataset=playwright&router=fixture-router&source=true',
		params: { slug: '202503010200' }
	}
];

describe('database drivers', () => {
	afterEach(() => {
		cloudflareEnv.DB = null;
		vi.doUnmock('#db');
		vi.unstubAllEnvs();
		vi.resetModules();
		for (const fixture of fixtures.splice(0)) {
			cleanupPlaywrightDatabase(fixture);
		}
	});

	it('export the same driver interface', async () => {
		vi.resetModules();
		const [{ default: sqlite }, { default: d1 }] = await Promise.all([
			import('../../../../src/lib/server/db/sqlite.ts'),
			import('../../../../src/lib/server/db/d1.ts')
		]);
		expect(Object.keys(d1).sort()).toEqual(Object.keys(sqlite).sort());
	});

	it.each(routes)(
		'return identical $name responses from SQLite and D1 blob rows',
		async ({ route, url, params }) => {
			const databasePath = seedDatabase();
			const sqlite = await callRoute('sqlite', databasePath, route, url, params);
			const d1 = await callRoute('d1', databasePath, route, url, params);

			expect(sqlite.status).toBe(200);
			expect(d1).toEqual(sqlite);
		}
	);
});
