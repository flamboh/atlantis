import Database from 'better-sqlite3';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { GET as getDimensionStats } from '../../src/routes/api/netflow/dimension-stats/+server';
import { GET as getMaadStatus } from '../../src/routes/api/netflow/maad-status/+server';
import { getRequestedDataset, withDatasetDb } from '#lib/server/datasets.ts';
import { localSchemaSql } from '#lib/server/db/local-schema.ts';

vi.mock('#lib/server/datasets.ts', () => ({
	getRequestedDataset: vi.fn(),
	withDatasetDb: vi.fn()
}));

const databases: Database.Database[] = [];

function openDatabase(): Database.Database {
	const database = new Database(':memory:');
	database.exec(localSchemaSql);
	databases.push(database);
	vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
	vi.mocked(withDatasetDb).mockImplementation(async (_datasetId, run) =>
		run({
			db: {
				get: async (query: string, params: unknown[] = []) =>
					database.prepare(query).get(...params),
				all: async (query: string, params: unknown[] = []) => database.prepare(query).all(...params)
			} as never,
			listSources: async () => [],
			listSourceDefinitions: async () => []
		})
	);
	return database;
}

function insertDimensions(
	database: Database.Database,
	row: {
		sourceId?: string;
		granularity?: string;
		bucketStart: number;
		ipVersion?: number;
		addressSide: 'source' | 'destination';
		measure: 'addresses' | 'packets' | 'bytes';
		dimensions: [number, number, number] | null;
	}
) {
	const tau = row.dimensions === null ? null : Buffer.alloc(4);
	database
		.prepare(
			`INSERT INTO address_maad_stats (
				source_id, granularity, bucket_start, bucket_end, ip_version, src_locality, dst_locality,
				address_side, measure, total_addrs, d0, d1, d2, tau, tau_sd
			) VALUES (?, ?, ?, ?, ?, 'all', 'all', ?, ?, 10, ?, ?, ?, ?, ?)`
		)
		.run(
			row.sourceId ?? 'r1',
			row.granularity ?? '1h',
			row.bucketStart,
			row.bucketStart + 3600,
			row.ipVersion ?? 4,
			row.addressSide,
			row.measure,
			row.dimensions?.[0] ?? null,
			row.dimensions?.[1] ?? null,
			row.dimensions?.[2] ?? null,
			tau,
			tau
		);
}

function insertCoverage(database: Database.Database, bucketStart: number) {
	database
		.prepare(
			`INSERT INTO bucket_coverage (
				source_id, granularity, bucket_start, bucket_end,
				coverage_state, observed_units, expected_units, rejected_units
			) VALUES ('r1', '1h', ?, ?, 'complete', 12, 12, 0)`
		)
		.run(bucketStart, bucketStart + 3600);
}

function dimensionUrl(search: string): URL {
	return new URL(`http://localhost/api/netflow/dimension-stats?${search}`);
}

afterEach(() => {
	for (const database of databases.splice(0)) {
		database.close();
	}
	vi.clearAllMocks();
});

describe('/api/netflow/dimension-stats', () => {
	it('returns stored D0, D1 and D2 for the requested measure over a half-open window', async () => {
		const database = openDatabase();
		for (const bucketStart of [0, 3600, 7200]) {
			insertCoverage(database, bucketStart);
			for (const measure of ['addresses', 'packets', 'bytes'] as const) {
				const offset = { addresses: 0, packets: 0.1, bytes: 0.2 }[measure] + bucketStart / 36000;
				insertDimensions(database, {
					bucketStart,
					addressSide: 'source',
					measure,
					dimensions: [1 + offset, 2 + offset, 3 + offset]
				});
				insertDimensions(database, {
					bucketStart,
					addressSide: 'destination',
					measure,
					dimensions: measure === 'bytes' && bucketStart === 3600 ? null : [4, 5, 6]
				});
			}
		}
		insertDimensions(database, {
			bucketStart: 0,
			ipVersion: 6,
			addressSide: 'source',
			measure: 'bytes',
			dimensions: [9, 9, 9]
		});
		insertDimensions(database, {
			bucketStart: 0,
			granularity: '1d',
			addressSide: 'source',
			measure: 'bytes',
			dimensions: [8, 8, 8]
		});

		const response = await getDimensionStats({
			url: dimensionUrl('routers=r1&granularity=1h&startDate=0&endDate=7200&measure=bytes')
		} as never);

		expect(response.status).toBe(200);
		await expect(response.json()).resolves.toEqual({
			timelines: [
				{
					router: 'r1',
					buckets: [
						{
							bucketStart: 0,
							bucketEnd: 3600,
							coverage: { state: 'complete', observedUnits: 12, expectedUnits: 12 },
							data: { saD0: 1.2, saD1: 2.2, saD2: 3.2, daD0: 4, daD1: 5, daD2: 6 }
						},
						{
							bucketStart: 3600,
							bucketEnd: 7200,
							coverage: { state: 'complete', observedUnits: 12, expectedUnits: 12 },
							data: { saD0: 1.3, saD1: 2.3, saD2: 3.3, daD0: null, daD1: null, daD2: null }
						}
					]
				}
			],
			requestedRouters: ['r1']
		});
	});

	it('defaults to the addresses measure and binds the requested ip version', async () => {
		const database = openDatabase();
		insertCoverage(database, 0);
		insertDimensions(database, {
			bucketStart: 0,
			addressSide: 'source',
			measure: 'addresses',
			dimensions: [1, 2, 3]
		});
		insertDimensions(database, {
			bucketStart: 0,
			ipVersion: 6,
			addressSide: 'source',
			measure: 'addresses',
			dimensions: [7, 8, 9]
		});

		const v4 = await getDimensionStats({
			url: dimensionUrl('routers=r1&granularity=1h&startDate=0&endDate=3600')
		} as never);
		const v6 = await getDimensionStats({
			url: dimensionUrl('routers=r1&granularity=1h&startDate=0&endDate=3600&ipVersion=6')
		} as never);

		expect((await v4.json()).timelines[0].buckets[0].data).toMatchObject({ saD0: 1, daD0: null });
		expect((await v6.json()).timelines[0].buckets[0].data).toMatchObject({ saD0: 7, saD2: 9 });
	});

	it('rejects invalid measure, ip version and window params', async () => {
		openDatabase();
		const cases: Array<[string, string]> = [
			[
				'routers=r1&startDate=0&endDate=3600&measure=flows',
				'Invalid measure. Expected one of: addresses, packets, bytes'
			],
			[
				'routers=r1&startDate=0&endDate=3600&ipVersion=5',
				'Invalid ipVersion. Expected one of: 4, 6'
			],
			['routers=r1&startDate=3600&endDate=3600', 'Start time must be before end time'],
			['startDate=0&endDate=3600', 'No routers selected']
		];

		for (const [search, error] of cases) {
			const response = await getDimensionStats({ url: dimensionUrl(search) } as never);
			expect(response.status).toBe(400);
			await expect(response.json()).resolves.toEqual({ error });
		}
		expect(withDatasetDb).not.toHaveBeenCalled();
	});
});

describe('/api/netflow/maad-status', () => {
	it('reports whether the product stored a MAAD q grid', async () => {
		const database = openDatabase();
		const url = new URL('http://localhost/api/netflow/maad-status?dataset=alpha');

		await expect((await getMaadStatus({ url } as never)).json()).resolves.toEqual({
			computed: false
		});

		database
			.prepare(
				'INSERT INTO maad_q_grid (ip_version, q_min, q_step, q_count) VALUES (4, -0.5, 0.125, 33)'
			)
			.run();
		await expect((await getMaadStatus({ url } as never)).json()).resolves.toEqual({
			computed: true
		});
	});
});
