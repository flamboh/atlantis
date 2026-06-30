import os from 'os';
import path from 'path';
import fs from 'fs';
import { spawnSync } from 'child_process';
import { afterEach, describe, expect, it, vi } from 'vitest';

async function loadDatasetsModule() {
	vi.resetModules();
	return import('../../../src/lib/server/datasets');
}

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
					CREATE TABLE traffic_stats_v3 (
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
					INSERT INTO traffic_stats_v3 (
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
				sourceCount: 2,
				isDefault: true
			}
		]);
		await expect(datasets.listDatasetSources('alpha')).resolves.toEqual(['router-a', 'router-b']);
		await expect(
			datasets.getRequestedDataset(new URL('http://localhost/api?dataset=alpha'))
		).resolves.toBe('alpha');
	});

	it('lists source member definitions from metadata', async () => {
		const dbPath = createSqliteFixture();
		const seedResult = spawnSync(
			'sqlite3',
			[
				dbPath,
				`
						INSERT INTO traffic_stats_v3 (
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

	it('infers source member definitions from processed nfcapd locators', async () => {
		const dbPath = createSqliteFixture();
		const seedResult = spawnSync(
			'sqlite3',
			[
				dbPath,
				`
						INSERT INTO traffic_stats_v3 (
							source_id, granularity, bucket_start, ip_version, src_visibility, dst_visibility
						) VALUES ('uoregon_all', '5m', 1740823200, 4, 'all', 'all');
					CREATE TABLE processed_inputs_v2 (
						input_kind TEXT NOT NULL,
						input_locator TEXT NOT NULL,
						source_id TEXT NOT NULL,
						bucket_start INTEGER NOT NULL,
						bucket_end INTEGER NOT NULL,
						status TEXT NOT NULL
					);
					INSERT INTO processed_inputs_v2 (
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
				sourceCount: 1,
				isDefault: true
			},
			{
				datasetId: 'beta',
				label: 'Beta',
				defaultStartDate: '2025-03-01',
				discoveryMode: 'static',
				sourceCount: 1,
				isDefault: false
			}
		]);
		await expect(datasets.listDatasetSources('beta')).resolves.toEqual(['router-b']);
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
});

function seedDatasetDb(dbPath: string, datasetId: string, label: string, sourceId: string): void {
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
					CREATE TABLE traffic_stats_v3 (
						source_id TEXT NOT NULL,
						granularity TEXT NOT NULL,
						bucket_start INTEGER NOT NULL,
						ip_version INTEGER NOT NULL,
						src_visibility TEXT NOT NULL,
						dst_visibility TEXT NOT NULL
					);
				INSERT INTO datasets (
					id,
					label,
					default_start_date,
					source_mode,
					discovery_mode,
					sort_order
				) VALUES ('${datasetId}', '${label}', '2025-03-01', 'static', 'static', 0);
					INSERT INTO traffic_stats_v3 (
						source_id, granularity, bucket_start, ip_version, src_visibility, dst_visibility
					) VALUES ('${sourceId}', '5m', 1740823200, 4, 'all', 'all');
			`
		],
		{ encoding: 'utf-8' }
	);
	expect(seedResult.status, seedResult.stderr).toBe(0);
}
