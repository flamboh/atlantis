import os from 'os';
import path from 'path';
import fs from 'fs';
import { spawnSync } from 'child_process';
import Database from 'better-sqlite3';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ReadonlyDatasetDb } from '../../../src/lib/server/datasets';

const coverageTableSql = `
	CREATE TABLE bucket_coverage (
		source_id TEXT NOT NULL,
		granularity TEXT NOT NULL,
		bucket_start INTEGER NOT NULL,
		bucket_end INTEGER NOT NULL,
		coverage_state TEXT NOT NULL,
		observed_units INTEGER NOT NULL,
		expected_units INTEGER NOT NULL,
		rejected_units INTEGER NOT NULL,
		PRIMARY KEY(source_id, granularity, bucket_start)
	);
`;

async function loadDatasetsModule() {
	vi.resetModules();
	return import('../../../src/lib/server/datasets');
}

type FakeD1DatasetRow = {
	id: string;
	label: string;
	defaultStartDate: string;
	discoveryMode: string;
	sortOrder: number;
};

function createD1Platform(initialRows: FakeD1DatasetRow[]) {
	let datasetRows = initialRows;
	const queries: Array<{ sql: string; params: unknown[] }> = [];
	const sourceMemberRows = [
		{ datasetId: 'alpha', sourceId: 'router-a', memberId: 'router-a' },
		{ datasetId: 'alpha', sourceId: 'router-b', memberId: 'router-b' }
	];
	const database = {
		prepare(sql: string) {
			return {
				bind(...params: unknown[]) {
					return {
						async all() {
							queries.push({ sql, params });
							if (sql.includes('FROM datasets')) {
								const requestedId = sql.includes('WHERE id = ?') ? params[0] : undefined;
								const results = datasetRows
									.filter((row) => requestedId === undefined || row.id === requestedId)
									.sort(
										(left, right) =>
											left.sortOrder - right.sortOrder || left.id.localeCompare(right.id)
									);
								return { results };
							}
							if (sql.includes('FROM source_members')) {
								return {
									results: sourceMemberRows
										.filter((row) => row.datasetId === params[0])
										.map(({ sourceId, memberId }) => ({ sourceId, memberId }))
								};
							}
							throw new Error(`Unexpected D1 query: ${sql}`);
						}
					};
				}
			};
		}
	};

	return {
		platform: { env: { DB: database } } as unknown as App.Platform,
		queries,
		setDatasetRows(rows: FakeD1DatasetRow[]) {
			datasetRows = rows;
		}
	};
}

const alphaD1Row: FakeD1DatasetRow = {
	id: 'alpha',
	label: 'Alpha Label',
	defaultStartDate: '2025-03-01',
	discoveryMode: 'static',
	sortOrder: 0
};

function createSqliteFixture(): string {
	const tempDir = os.tmpdir();
	const dbPath = path.join(tempDir, `datasets-test-${crypto.randomUUID()}.sqlite`);
	const seedResult = spawnSync(
		'sqlite3',
		[
			dbPath,
			`
				CREATE TABLE datasets (
					id TEXT PRIMARY KEY NOT NULL,
					label TEXT NOT NULL,
					default_start_date TEXT NOT NULL,
					source_mode TEXT DEFAULT 'static' NOT NULL,
					discovery_mode TEXT DEFAULT 'static' NOT NULL,
					sort_order INTEGER DEFAULT 0 NOT NULL
				);
				${coverageTableSql}
				CREATE TABLE traffic_stats (
					source_id TEXT NOT NULL,
					granularity TEXT NOT NULL,
					bucket_start INTEGER NOT NULL,
					ip_version INTEGER NOT NULL,
					src_visibility TEXT NOT NULL,
					dst_visibility TEXT NOT NULL
				);
				CREATE TABLE source_members (
					dataset_id TEXT NOT NULL,
					source_id TEXT NOT NULL,
					member_id TEXT NOT NULL,
					PRIMARY KEY(dataset_id, source_id, member_id)
				);
				INSERT INTO datasets (
					id,
					label,
					default_start_date,
					source_mode,
					discovery_mode,
					sort_order
				) VALUES ('alpha', 'Alpha Label', '2025-03-01', 'static', 'static', 0);
				INSERT INTO traffic_stats (
					source_id, granularity, bucket_start, ip_version, src_visibility, dst_visibility
				) VALUES
					('router-b', '5m', 1740823200, 4, 'all', 'all'),
					('router-a', '5m', 1740823200, 4, 'all', 'all');
			`
		],
		{ encoding: 'utf-8' }
	);
	expect(seedResult.status, seedResult.stderr).toBe(0);
	return dbPath;
}

describe('dataset server helpers', () => {
	const originalCwd = process.cwd();

	afterEach(() => {
		process.chdir(originalCwd);
		vi.unstubAllEnvs();
		vi.doUnmock('node:fs/promises');
	});

	it('lists dataset summaries from local sqlite metadata', async () => {
		vi.stubEnv('LOCAL_SQLITE_PATH', createSqliteFixture());
		vi.stubEnv('DEFAULT_DATASET', 'alpha');

		const datasets = await loadDatasetsModule();

		await expect(datasets.listDatasetSummaries()).resolves.toEqual([
			{
				datasetId: 'alpha',
				label: 'Alpha Label',
				defaultStartDate: '2025-03-01',
				discoveryMode: 'static',
				isDefault: true
			}
		]);
		await expect(datasets.listDatasetSources('alpha')).resolves.toEqual(['router-a', 'router-b']);
		await expect(
			datasets.getRequestedDataset(new URL('http://localhost/api?dataset=alpha'))
		).resolves.toBe('alpha');
	});

	it('lists dataset summaries without reading traffic statistics', async () => {
		const dbPath = createSqliteFixture();
		const dropResult = spawnSync('sqlite3', [dbPath, 'DROP TABLE traffic_stats;'], {
			encoding: 'utf-8'
		});
		expect(dropResult.status, dropResult.stderr).toBe(0);
		vi.stubEnv('LOCAL_SQLITE_PATH', dbPath);

		const datasets = await loadDatasetsModule();

		await expect(datasets.listDatasetSummaries()).resolves.toEqual([
			{
				datasetId: 'alpha',
				label: 'Alpha Label',
				defaultStartDate: '2025-03-01',
				discoveryMode: 'static',
				isDefault: true
			}
		]);
	});

	it('loads the D1 catalog once when building summaries', async () => {
		const fake = createD1Platform([
			alphaD1Row,
			{ ...alphaD1Row, id: 'beta', label: 'Beta Label', sortOrder: 1 }
		]);
		vi.stubEnv('DEFAULT_DATASET', 'beta');
		const datasets = await loadDatasetsModule();

		await expect(datasets.listDatasetSummaries(fake.platform)).resolves.toEqual([
			{
				datasetId: 'alpha',
				label: 'Alpha Label',
				defaultStartDate: '2025-03-01',
				discoveryMode: 'static',
				isDefault: false
			},
			{
				datasetId: 'beta',
				label: 'Beta Label',
				defaultStartDate: '2025-03-01',
				discoveryMode: 'static',
				isDefault: true
			}
		]);
		expect(fake.queries).toHaveLength(1);
		expect(fake.queries[0]?.sql).toContain('FROM datasets');
		expect(fake.queries[0]?.sql).not.toContain('WHERE id = ?');
	});

	it('resolves an explicit D1 dataset with one targeted query', async () => {
		const fake = createD1Platform([alphaD1Row]);
		const datasets = await loadDatasetsModule();

		await expect(
			datasets.getRequestedDataset(
				new URL('http://localhost/api/netflow/stats?dataset=alpha'),
				fake.platform
			)
		).resolves.toBe('alpha');
		expect(fake.queries).toHaveLength(1);
		expect(fake.queries[0]?.sql).toContain('WHERE id = ?');
		expect(fake.queries[0]?.params).toEqual(['alpha']);
	});

	it('does not cache D1 dataset metadata across calls', async () => {
		const fake = createD1Platform([alphaD1Row]);
		const datasets = await loadDatasetsModule();

		await expect(datasets.getDatasetLabel('alpha', fake.platform)).resolves.toBe('Alpha Label');
		fake.setDatasetRows([{ ...alphaD1Row, label: 'Updated Alpha' }]);
		await expect(datasets.getDatasetLabel('alpha', fake.platform)).resolves.toBe('Updated Alpha');
		expect(fake.queries).toHaveLength(2);
		expect(fake.queries.every((query) => query.sql.includes('WHERE id = ?'))).toBe(true);
	});

	it('preserves available dataset details for an unknown D1 dataset', async () => {
		const fake = createD1Platform([alphaD1Row]);
		const datasets = await loadDatasetsModule();

		await expect(datasets.getDatasetConfig('missing', fake.platform)).rejects.toThrow(
			"Unknown dataset 'missing'. Available datasets: alpha"
		);
		expect(fake.queries).toHaveLength(2);
		expect(fake.queries[0]?.sql).toContain('WHERE id = ?');
		expect(fake.queries[1]?.sql).not.toContain('WHERE id = ?');
	});

	it('opens local dataset databases as strictly readonly', async () => {
		const dbPath = createSqliteFixture();
		vi.stubEnv('LOCAL_SQLITE_PATH', dbPath);

		const datasets = await loadDatasetsModule();
		await datasets.withDatasetDb('alpha', undefined, async ({ db }) => {
			await expect(db.get('DELETE FROM datasets RETURNING id')).rejects.toThrow(/readonly/i);
		});
		await expect(datasets.getDatasetConfig('alpha')).resolves.toMatchObject({ id: 'alpha' });
	});

	it('does not initialize schema or inferred metadata while opening a request database', async () => {
		const dbPath = path.join(os.tmpdir(), `datasets-empty-${crypto.randomUUID()}.sqlite`);
		const seedResult = spawnSync('sqlite3', [dbPath, 'CREATE TABLE placeholder (id INTEGER);'], {
			encoding: 'utf-8'
		});
		expect(seedResult.status, seedResult.stderr).toBe(0);
		vi.stubEnv('LOCAL_SQLITE_PATH', dbPath);

		const datasets = await loadDatasetsModule();

		await expect(datasets.listDatasetSummaries()).rejects.toThrow(/no such table: datasets/i);
		const schemaResult = spawnSync(
			'sqlite3',
			[dbPath, "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'datasets';"],
			{ encoding: 'utf-8' }
		);
		expect(schemaResult.status, schemaResult.stderr).toBe(0);
		expect(schemaResult.stdout.trim()).toBe('0');
	});

	it('lists source member definitions from metadata', async () => {
		const dbPath = createSqliteFixture();
		const seedResult = spawnSync(
			'sqlite3',
			[
				dbPath,
				`
						INSERT INTO traffic_stats (
							source_id, granularity, bucket_start, ip_version, src_visibility, dst_visibility
						) VALUES ('uoregon_all', '5m', 1740823200, 4, 'all', 'all');
					INSERT INTO source_members (dataset_id, source_id, member_id)
					VALUES
						('alpha', 'router-a', 'router-a'),
						('alpha', 'router-b', 'router-b'),
						('alpha', 'uoregon_all', 'router-a'),
						('alpha', 'uoregon_all', 'router-b');
				`
			],
			{ encoding: 'utf-8' }
		);
		expect(seedResult.status, seedResult.stderr).toBe(0);
		vi.stubEnv('LOCAL_SQLITE_PATH', dbPath);

		const datasets = await loadDatasetsModule();

		await expect(datasets.listDatasetSourceDefinitions('alpha')).resolves.toEqual([
			{ sourceId: 'router-a', members: ['router-a'] },
			{ sourceId: 'router-b', members: ['router-b'] },
			{ sourceId: 'uoregon_all', members: ['router-a', 'router-b'] }
		]);
	});

	it('uses configured source members without scanning traffic statistics', async () => {
		const dbPath = createSqliteFixture();
		const seedResult = spawnSync(
			'sqlite3',
			[
				dbPath,
				`
					INSERT INTO source_members (dataset_id, source_id, member_id)
					VALUES
						('alpha', 'router-a', 'router-a'),
						('alpha', 'router-b', 'router-b');
					DROP TABLE traffic_stats;
				`
			],
			{ encoding: 'utf-8' }
		);
		expect(seedResult.status, seedResult.stderr).toBe(0);
		vi.stubEnv('LOCAL_SQLITE_PATH', dbPath);
		const datasets = await loadDatasetsModule();

		await expect(datasets.listDatasetSources('alpha')).resolves.toEqual(['router-a', 'router-b']);
		await expect(datasets.listDatasetSourceDefinitions('alpha')).resolves.toEqual([
			{ sourceId: 'router-a', members: ['router-a'] },
			{ sourceId: 'router-b', members: ['router-b'] }
		]);
	});

	it('uses one D1 source-members query for configured definitions', async () => {
		const fake = createD1Platform([alphaD1Row]);
		const datasets = await loadDatasetsModule();

		await expect(datasets.listDatasetSourceDefinitions('alpha', fake.platform)).resolves.toEqual([
			{ sourceId: 'router-a', members: ['router-a'] },
			{ sourceId: 'router-b', members: ['router-b'] }
		]);
		expect(fake.queries).toHaveLength(1);
		expect(fake.queries[0]?.sql).toContain('FROM source_members');
		expect(fake.queries.some((query) => query.sql.includes('FROM traffic_stats'))).toBe(false);
	});

	it('infers source member definitions from processed nfcapd locators', async () => {
		const dbPath = createSqliteFixture();
		const seedResult = spawnSync(
			'sqlite3',
			[
				dbPath,
				`
						INSERT INTO traffic_stats (
							source_id, granularity, bucket_start, ip_version, src_visibility, dst_visibility
						) VALUES ('uoregon_all', '5m', 1740823200, 4, 'all', 'all');
					CREATE TABLE processed_inputs (
						input_kind TEXT NOT NULL,
						input_locator TEXT NOT NULL,
						source_id TEXT NOT NULL,
						bucket_start INTEGER NOT NULL,
						bucket_end INTEGER NOT NULL,
						status TEXT NOT NULL
					);
					INSERT INTO processed_inputs (
						input_kind,
						input_locator,
						source_id,
						bucket_start,
						bucket_end,
						status
					) VALUES
						('nfcapd', '/data/cc_ir1_gw/2025/03/01/nfcapd.202503010000', 'uoregon_all', 1, 2, 'processed'),
						('nfcapd', '/data/oh_ir1_gw/2025/03/01/nfcapd.202503010000', 'uoregon_all', 1, 2, 'processed');
				`
			],
			{ encoding: 'utf-8' }
		);
		expect(seedResult.status, seedResult.stderr).toBe(0);
		vi.stubEnv('LOCAL_SQLITE_PATH', dbPath);

		const datasets = await loadDatasetsModule();

		await expect(datasets.listDatasetSourceDefinitions('alpha')).resolves.toEqual([
			{ sourceId: 'router-a', members: ['router-a'] },
			{ sourceId: 'router-b', members: ['router-b'] },
			{ sourceId: 'uoregon_all', members: ['cc_ir1_gw', 'oh_ir1_gw'] }
		]);
	});

	it('rejects unknown datasets', async () => {
		vi.stubEnv('LOCAL_SQLITE_PATH', createSqliteFixture());

		const datasets = await loadDatasetsModule();

		await expect(datasets.getDatasetConfig('missing')).rejects.toThrow(/Unknown dataset 'missing'/);
	});

	it('returns an empty summary list when no local databases exist', async () => {
		const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'datasets-none-'));
		fs.mkdirSync(path.join(workspace, 'data'));
		process.chdir(workspace);

		const datasets = await loadDatasetsModule();

		await expect(datasets.listDatasetSummaries()).resolves.toEqual([]);
	});

	it('discovers local sqlite datasets from data directories', async () => {
		const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'datasets-scan-'));
		const alphaDir = path.join(workspace, 'data', 'alpha');
		const betaDir = path.join(workspace, 'data', 'beta');
		fs.mkdirSync(alphaDir, { recursive: true });
		fs.mkdirSync(betaDir, { recursive: true });
		seedDatasetDb(path.join(alphaDir, 'netflow.sqlite'), 'alpha', 'Alpha', 'router-a');
		seedDatasetDb(path.join(betaDir, 'netflow.sqlite'), 'beta', 'Beta', 'router-b');
		process.chdir(workspace);

		const datasets = await loadDatasetsModule();

		await expect(datasets.listDatasetSummaries()).resolves.toEqual([
			{
				datasetId: 'alpha',
				label: 'Alpha',
				defaultStartDate: '2025-03-01',
				discoveryMode: 'static',
				isDefault: true
			},
			{
				datasetId: 'beta',
				label: 'Beta',
				defaultStartDate: '2025-03-01',
				discoveryMode: 'static',
				isDefault: false
			}
		]);
		await expect(datasets.listDatasetSources('beta')).resolves.toEqual(['router-b']);
	});

	it('ignores obsolete auto-discovered databases that reuse a current dataset id', async () => {
		const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'datasets-schema-filter-'));
		const oldDir = path.join(workspace, 'data', 'uoregon-v2');
		const currentDir = path.join(workspace, 'data', 'uoregon-v4');
		fs.mkdirSync(oldDir, { recursive: true });
		fs.mkdirSync(currentDir, { recursive: true });
		seedDatasetDb(
			path.join(oldDir, 'netflow.sqlite'),
			'uoregon',
			'Obsolete UOregon',
			'router-old',
			false
		);
		seedDatasetDb(
			path.join(currentDir, 'netflow.sqlite'),
			'uoregon',
			'Current UOregon',
			'router-current'
		);
		process.chdir(workspace);

		const datasets = await loadDatasetsModule();

		await expect(datasets.listDatasetSummaries()).resolves.toEqual([
			{
				datasetId: 'uoregon',
				label: 'Current UOregon',
				defaultStartDate: '2025-03-01',
				discoveryMode: 'static',
				isDefault: true
			}
		]);
		await expect(datasets.getDatasetLabel('uoregon')).resolves.toBe('Current UOregon');
	});

	it('refreshes local dataset discovery after files move', async () => {
		const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'datasets-refresh-'));
		const alphaDir = path.join(workspace, 'data', 'alpha');
		const archivedDir = path.join(workspace, 'data', '_archive', 'alpha');
		const betaDir = path.join(workspace, 'data', 'beta');
		fs.mkdirSync(alphaDir, { recursive: true });
		seedDatasetDb(path.join(alphaDir, 'netflow.sqlite'), 'alpha', 'Alpha', 'router-a');
		process.chdir(workspace);

		const datasets = await loadDatasetsModule();

		await expect(datasets.listDatasets()).resolves.toMatchObject([{ id: 'alpha' }]);

		fs.mkdirSync(path.dirname(archivedDir), { recursive: true });
		fs.renameSync(alphaDir, archivedDir);
		fs.mkdirSync(betaDir, { recursive: true });
		seedDatasetDb(path.join(betaDir, 'netflow.sqlite'), 'beta', 'Beta', 'router-b');

		await expect(datasets.listDatasets()).resolves.toMatchObject([{ id: 'beta' }]);
		await expect(datasets.getDatasetConfig('alpha')).rejects.toThrow(/Unknown dataset 'alpha'/);
	});

	it('reopens a cached readonly database after atomic file replacement', async () => {
		const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'datasets-replace-'));
		const dbPath = path.join(workspace, 'netflow.sqlite');
		const replacementPath = path.join(workspace, 'replacement.sqlite');
		seedDatasetDb(dbPath, 'alpha', 'Before replacement', 'router-a');
		vi.stubEnv('LOCAL_SQLITE_PATH', dbPath);

		const datasets = await loadDatasetsModule();
		await expect(datasets.getDatasetLabel('alpha')).resolves.toBe('Before replacement');

		seedDatasetDb(replacementPath, 'alpha', 'After replacement', 'router-a');
		fs.renameSync(replacementPath, dbPath);

		await expect(datasets.getDatasetLabel('alpha')).resolves.toBe('After replacement');
	});

	it('refreshes cached source metadata after atomic file replacement', async () => {
		const dbPath = createSqliteFixture();
		const replacementPath = createSqliteFixture();
		for (const [target, sourceId] of [
			[dbPath, 'router-a'],
			[replacementPath, 'router-c']
		] as const) {
			const seedResult = spawnSync(
				'sqlite3',
				[
					target,
					`INSERT INTO source_members (dataset_id, source_id, member_id)
					 VALUES ('alpha', '${sourceId}', '${sourceId}');`
				],
				{ encoding: 'utf-8' }
			);
			expect(seedResult.status, seedResult.stderr).toBe(0);
		}
		vi.stubEnv('LOCAL_SQLITE_PATH', dbPath);
		const datasets = await loadDatasetsModule();

		await expect(datasets.listDatasetSources('alpha')).resolves.toEqual(['router-a']);
		fs.renameSync(replacementPath, dbPath);
		await expect(datasets.listDatasetSources('alpha')).resolves.toEqual(['router-c']);
	});

	it('refreshes cached dataset and source metadata after a WAL commit', async () => {
		const dbPath = createSqliteFixture();
		const writer = new Database(dbPath);
		writer.pragma('journal_mode = WAL');
		writer.pragma('wal_autocheckpoint = 0');
		try {
			writer
				.prepare(
					`INSERT INTO source_members (dataset_id, source_id, member_id)
					 VALUES ('alpha', 'router-a', 'router-a'), ('alpha', 'router-b', 'router-b')`
				)
				.run();
			const mainBeforeRead = fs.statSync(dbPath);
			const walBeforeRead = fs.statSync(`${dbPath}-wal`);
			vi.stubEnv('LOCAL_SQLITE_PATH', dbPath);
			const datasets = await loadDatasetsModule();

			await expect(datasets.getDatasetLabel('alpha')).resolves.toBe('Alpha Label');
			await expect(datasets.listDatasetSources('alpha')).resolves.toEqual(['router-a', 'router-b']);

			writer.transaction(() => {
				writer.prepare(`UPDATE datasets SET label = 'WAL Label' WHERE id = 'alpha'`).run();
				writer.prepare(`DELETE FROM source_members WHERE dataset_id = 'alpha'`).run();
				writer
					.prepare(
						`INSERT INTO source_members (dataset_id, source_id, member_id)
						 VALUES ('alpha', 'router-wal', 'router-wal')`
					)
					.run();
			})();

			const mainAfterCommit = fs.statSync(dbPath);
			const walAfterCommit = fs.statSync(`${dbPath}-wal`);
			expect(mainAfterCommit.dev).toBe(mainBeforeRead.dev);
			expect(mainAfterCommit.ino).toBe(mainBeforeRead.ino);
			expect(mainAfterCommit.size).toBe(mainBeforeRead.size);
			expect(mainAfterCommit.mtimeMs).toBe(mainBeforeRead.mtimeMs);
			expect(
				walAfterCommit.size !== walBeforeRead.size ||
					walAfterCommit.mtimeMs !== walBeforeRead.mtimeMs
			).toBe(true);

			await expect(datasets.getDatasetLabel('alpha')).resolves.toBe('WAL Label');
			await expect(datasets.listDatasetSources('alpha')).resolves.toEqual(['router-wal']);
		} finally {
			writer.close();
		}
	});

	it('keeps a request database open when another request detects an atomic replacement', async () => {
		const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'datasets-request-replace-'));
		const dbPath = path.join(workspace, 'netflow.sqlite');
		const replacementPath = path.join(workspace, 'replacement.sqlite');
		seedDatasetDb(dbPath, 'alpha', 'Before replacement', 'router-a');
		seedDatasetDb(replacementPath, 'alpha', 'After replacement', 'router-b');
		vi.stubEnv('LOCAL_SQLITE_PATH', dbPath);
		const datasets = await loadDatasetsModule();
		let retiredDb: ReadonlyDatasetDb | undefined;

		await datasets.withDatasetDb('alpha', undefined, async ({ db: requestDb, listSources }) => {
			retiredDb = requestDb;
			fs.renameSync(replacementPath, dbPath);
			await expect(datasets.listDatasetSources('alpha')).resolves.toEqual(['router-b']);
			await expect(listSources()).resolves.toEqual(['router-a']);
			await expect(
				requestDb.get<{ label: string }>('SELECT label FROM datasets WHERE id = ?', ['alpha'])
			).resolves.toEqual({ label: 'Before replacement' });
		});
		await expect(retiredDb?.get('SELECT 1')).rejects.toThrow(/connection is not open/i);
	});

	it('retries when the database is atomically replaced while opening it', async () => {
		const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'datasets-open-race-'));
		const dbPath = path.join(workspace, 'netflow.sqlite');
		const intermediatePath = path.join(workspace, 'intermediate.sqlite');
		const finalPath = path.join(workspace, 'final.sqlite');
		seedDatasetDb(dbPath, 'alpha', 'Initially cached', 'router-a');
		vi.stubEnv('LOCAL_SQLITE_PATH', dbPath);
		const fsPromises = await vi.importActual<typeof import('node:fs/promises')>('node:fs/promises');
		let raceEnabled = false;
		let statCalls = 0;
		vi.doMock('node:fs/promises', () => ({
			...fsPromises,
			stat: async (...args: Parameters<typeof fsPromises.stat>) => {
				if (raceEnabled) {
					statCalls += 1;
					if (statCalls === 3) {
						fs.renameSync(finalPath, dbPath);
					}
				}
				return fsPromises.stat(...args);
			}
		}));

		const datasets = await loadDatasetsModule();
		await expect(datasets.getDatasetLabel('alpha')).resolves.toBe('Initially cached');

		seedDatasetDb(intermediatePath, 'alpha', 'Intermediate replacement', 'router-a');
		seedDatasetDb(finalPath, 'alpha', 'Final replacement', 'router-a');
		fs.renameSync(intermediatePath, dbPath);
		raceEnabled = true;

		await expect(datasets.getDatasetLabel('alpha')).resolves.toBe('Final replacement');
		expect(statCalls).toBeGreaterThanOrEqual(5);
	});
});

function seedDatasetDb(
	dbPath: string,
	datasetId: string,
	label: string,
	sourceId: string,
	includeCoverage = true
): void {
	const seedResult = spawnSync(
		'sqlite3',
		[
			dbPath,
			`
				CREATE TABLE datasets (
					id TEXT PRIMARY KEY NOT NULL,
					label TEXT NOT NULL,
					default_start_date TEXT NOT NULL,
					source_mode TEXT DEFAULT 'static' NOT NULL,
					discovery_mode TEXT DEFAULT 'static' NOT NULL,
					sort_order INTEGER DEFAULT 0 NOT NULL
				);
				CREATE TABLE traffic_stats (
					source_id TEXT NOT NULL,
					granularity TEXT NOT NULL,
					bucket_start INTEGER NOT NULL,
					ip_version INTEGER NOT NULL,
					src_visibility TEXT NOT NULL,
					dst_visibility TEXT NOT NULL
				);
				${includeCoverage ? coverageTableSql : ''}
				INSERT INTO datasets (
					id,
					label,
					default_start_date,
					source_mode,
					discovery_mode,
					sort_order
				) VALUES ('${datasetId}', '${label}', '2025-03-01', 'static', 'static', 0);
				INSERT INTO traffic_stats (
					source_id, granularity, bucket_start, ip_version, src_visibility, dst_visibility
				) VALUES ('${sourceId}', '5m', 1740823200, 4, 'all', 'all');
			`
		],
		{ encoding: 'utf-8' }
	);
	expect(seedResult.status, seedResult.stderr).toBe(0);
}
