import type { D1Database } from '@cloudflare/workers-types';
import { env as privateEnv } from '$env/dynamic/private';
import { localSchemaSql } from '$lib/server/db/local-schema';
import type { DatasetSummary } from '$lib/types/types';

type QueryParam = string | number | boolean | null | Uint8Array;

type DatasetRow = {
	id: string;
	label: string;
	defaultStartDate: string;
	discoveryMode: string;
	sortOrder: number;
};

type LocalDatasetRow = DatasetRow & {
	dbPath: string;
};

export type SourceDefinition = {
	sourceId: string;
	members: string[];
};

type SqliteClient = {
	exec(sql: string): unknown;
	prepare(sql: string): {
		get(...params: QueryParam[]): unknown;
		all(...params: QueryParam[]): unknown[];
		run(...params: QueryParam[]): unknown;
	};
};

export type PreparedStatement = {
	get<T = unknown>(...params: QueryParam[]): Promise<T | undefined>;
	all<T = unknown>(...params: QueryParam[]): Promise<T[]>;
};

export interface ReadonlyDatasetDb {
	get<T = unknown>(query: string, params?: QueryParam[]): Promise<T | undefined>;
	all<T = unknown>(query: string, params?: QueryParam[]): Promise<T[]>;
	prepare(sql: string): PreparedStatement;
}

const localDbCache = new Map<string, ReadonlyDatasetDb>();

function getEnv(name: string): string | undefined {
	return globalThis.process?.env?.[name]?.trim() || privateEnv[name]?.trim() || undefined;
}

function formatLocalDate(timestampSeconds: number): string {
	const date = new Date(timestampSeconds * 1000);
	const year = date.getFullYear();
	const month = `${date.getMonth() + 1}`.padStart(2, '0');
	const day = `${date.getDate()}`.padStart(2, '0');
	return `${year}-${month}-${day}`;
}

function makePrepared(db: ReadonlyDatasetDb, query: string): PreparedStatement {
	return {
		get: <T = unknown>(...params: QueryParam[]) => db.get<T>(query, params),
		all: <T = unknown>(...params: QueryParam[]) => db.all<T>(query, params)
	};
}

function createReadonlyDb(client: SqliteClient): ReadonlyDatasetDb {
	const db: ReadonlyDatasetDb = {
		async get<T = unknown>(query: string, params: QueryParam[] = []) {
			return client.prepare(query).get(...params) as T | undefined;
		},
		async all<T = unknown>(query: string, params: QueryParam[] = []) {
			return client.prepare(query).all(...params) as T[];
		},
		prepare(query: string) {
			return makePrepared(db, query);
		}
	};

	return db;
}

function createD1Db(d1: D1Database): ReadonlyDatasetDb {
	const db: ReadonlyDatasetDb = {
		async get<T = unknown>(query: string, params: QueryParam[] = []) {
			const result = await d1
				.prepare(query)
				.bind(...params)
				.all<T>();
			return result.results[0];
		},
		async all<T = unknown>(query: string, params: QueryParam[] = []) {
			const result = await d1
				.prepare(query)
				.bind(...params)
				.all<T>();
			return [...result.results];
		},
		prepare(query: string) {
			return makePrepared(db, query);
		}
	};

	return db;
}

function shouldUseD1(platform?: App.Platform): boolean {
	return getEnv('ATLANTIS_DB_DRIVER') !== 'sqlite' && Boolean(platform?.env.DB);
}

async function resolvePath(value: string): Promise<string> {
	if (value === ':memory:') {
		return value;
	}

	const path = await import('node:path');
	return path.isAbsolute(value) ? value : path.resolve(process.cwd(), value);
}

async function discoverLocalSqlitePaths(): Promise<string[]> {
	const configured = getEnv('LOCAL_SQLITE_PATH') ?? getEnv('DATABASE_PATH');
	if (configured) {
		return [await resolvePath(configured)];
	}

	const fs = await import('node:fs/promises');
	const path = await import('node:path');
	const roots = [path.resolve(process.cwd(), 'data'), path.resolve(process.cwd(), '../../data')];
	const dbPaths = new Set<string>();

	for (const root of roots) {
		let entries: import('node:fs').Dirent[];
		try {
			entries = await fs.readdir(root, { withFileTypes: true });
		} catch {
			continue;
		}

		for (const entry of entries) {
			if (!entry.isDirectory()) {
				continue;
			}
			const dbPath = path.join(root, entry.name, 'netflow.sqlite');
			try {
				const stat = await fs.stat(dbPath);
				if (stat.isFile()) {
					dbPaths.add(dbPath);
				}
			} catch {
				// Not every data directory is a web dataset.
			}
		}
	}

	if (dbPaths.size === 0) {
		throw new Error('No local SQLite datasets found under data/*/netflow.sqlite');
	}

	return [...dbPaths].sort();
}

async function inferDatasetIdFromPath(dbPath: string): Promise<string> {
	if (dbPath === ':memory:') {
		return 'ugr16';
	}

	const path = await import('node:path');
	return path.basename(path.dirname(dbPath));
}

function inferDatasetLabel(datasetId: string): string {
	if (datasetId === 'ugr16') {
		return 'UGR16';
	}
	return datasetId
		.split(/[-_]/)
		.filter(Boolean)
		.map((part) => part[0]?.toUpperCase() + part.slice(1))
		.join(' ');
}

function inferDefaultStartDate(client: SqliteClient): string {
	const row = client
		.prepare(
			"SELECT MIN(bucket_start) AS minTimestamp FROM traffic_stats_v3 WHERE granularity = '5m'"
		)
		.get() as { minTimestamp: number | null } | undefined;

	if (typeof row?.minTimestamp === 'number' && Number.isFinite(row.minTimestamp)) {
		return formatLocalDate(row.minTimestamp);
	}

	return '2025-02-01';
}

async function seedInferredDatasetMetadata(client: SqliteClient, dbPath: string): Promise<void> {
	const row = client.prepare('SELECT COUNT(*) AS count FROM datasets').get() as
		| { count: number }
		| undefined;
	if ((row?.count ?? 0) > 0) {
		return;
	}

	const datasetId = await inferDatasetIdFromPath(dbPath);
	client
		.prepare(
			`
				INSERT INTO datasets (
					id,
					label,
					default_start_date,
					source_mode,
					discovery_mode,
					sort_order
				) VALUES (?, ?, ?, 'static', 'live', 0)
			`
		)
		.run(datasetId, inferDatasetLabel(datasetId), inferDefaultStartDate(client));
}

async function openLocalClient(dbPath: string): Promise<SqliteClient> {
	const [{ drizzle }, betterSqlite3, schema] = await Promise.all([
		import(/* @vite-ignore */ 'drizzle-orm/better-sqlite3'),
		import(/* @vite-ignore */ 'better-sqlite3'),
		import('$lib/server/db/schema')
	]);
	const sqlite = new betterSqlite3.default(dbPath);
	const drizzleDb = drizzle(sqlite, { schema });
	return drizzleDb.$client as SqliteClient;
}

async function createLocalDb(dbPath: string): Promise<ReadonlyDatasetDb> {
	const client = await openLocalClient(dbPath);
	client.exec(localSchemaSql);
	await seedInferredDatasetMetadata(client, dbPath);
	return createReadonlyDb(client);
}

async function getLocalDb(dbPath: string): Promise<ReadonlyDatasetDb> {
	const existing = localDbCache.get(dbPath);
	if (existing) {
		return existing;
	}

	const db = await createLocalDb(dbPath);
	localDbCache.set(dbPath, db);
	return db;
}

async function readDatasetRowsFromDb(dbPath: string): Promise<LocalDatasetRow[]> {
	const db = await getLocalDb(dbPath);
	const rows = await db.all<DatasetRow>(
		`
			SELECT
				id,
				label,
				default_start_date AS defaultStartDate,
				discovery_mode AS discoveryMode,
				sort_order AS sortOrder
			FROM datasets
			ORDER BY sort_order ASC, id ASC
		`
	);

	return rows.map((row) => ({ ...row, dbPath }));
}

async function listLocalDatasetRows(): Promise<LocalDatasetRow[]> {
	const dbPaths = await discoverLocalSqlitePaths();
	const datasets = (await Promise.all(dbPaths.map(readDatasetRowsFromDb)))
		.flat()
		.sort((left, right) => left.sortOrder - right.sortOrder || left.id.localeCompare(right.id));

	if (datasets.length === 0) {
		throw new Error('No local datasets configured in discovered SQLite databases');
	}

	return datasets;
}

async function getLocalDatasetRow(datasetId: string): Promise<LocalDatasetRow> {
	const datasets = await listLocalDatasetRows();
	const dataset = datasets.find((item) => item.id === datasetId);
	if (!dataset) {
		const available = datasets.map((item) => item.id).join(', ');
		throw new Error(`Unknown dataset '${datasetId}'. Available datasets: ${available}`);
	}
	return dataset;
}

async function listD1DatasetRows(platform: App.Platform): Promise<DatasetRow[]> {
	const db = createD1Db(platform.env.DB);
	return db.all<DatasetRow>(
		`
			SELECT
				id,
				label,
				default_start_date AS defaultStartDate,
				discovery_mode AS discoveryMode,
				sort_order AS sortOrder
			FROM datasets
			ORDER BY sort_order ASC, id ASC
		`
	);
}

async function listDatasetRows(platform?: App.Platform): Promise<DatasetRow[]> {
	if (shouldUseD1(platform)) {
		return listD1DatasetRows(platform as App.Platform);
	}

	return listLocalDatasetRows();
}

export async function listDatasets(platform?: App.Platform): Promise<DatasetRow[]> {
	return listDatasetRows(platform);
}

export async function getDefaultDatasetId(platform?: App.Platform): Promise<string> {
	const configured = getEnv('DEFAULT_DATASET');
	const datasets = await listDatasetRows(platform);
	if (configured && datasets.some((dataset) => dataset.id === configured)) {
		return configured;
	}

	const firstDataset = datasets[0];
	if (!firstDataset) {
		throw new Error('No datasets configured');
	}

	return firstDataset.id;
}

export async function getDatasetConfig(
	datasetId: string,
	platform?: App.Platform
): Promise<DatasetRow> {
	if (!shouldUseD1(platform)) {
		return getLocalDatasetRow(datasetId);
	}

	const datasets = await listD1DatasetRows(platform as App.Platform);
	const dataset = datasets.find((item) => item.id === datasetId);
	if (!dataset) {
		const available = datasets.map((item) => item.id).join(', ');
		throw new Error(`Unknown dataset '${datasetId}'. Available datasets: ${available}`);
	}

	return dataset;
}

export async function getDatasetLabel(datasetId: string, platform?: App.Platform): Promise<string> {
	const dataset = await getDatasetConfig(datasetId, platform);
	return dataset.label.trim() || dataset.id;
}

export async function getDatasetDb(
	datasetOrPlatform?: string | App.Platform,
	platform?: App.Platform
): Promise<ReadonlyDatasetDb> {
	const datasetId = typeof datasetOrPlatform === 'string' ? datasetOrPlatform : undefined;
	const requestPlatform = typeof datasetOrPlatform === 'string' ? platform : datasetOrPlatform;

	if (shouldUseD1(requestPlatform)) {
		return createD1Db((requestPlatform as App.Platform).env.DB);
	}

	const resolvedDatasetId = datasetId ?? (await getDefaultDatasetId(requestPlatform));
	const dataset = await getLocalDatasetRow(resolvedDatasetId);
	return getLocalDb(dataset.dbPath);
}

export async function listDatasetSources(
	datasetId: string,
	platform?: App.Platform
): Promise<string[]> {
	const db = await getDatasetDb(datasetId, platform);
	const rows = await db.all<{ sourceId: string }>(
		`
			SELECT DISTINCT source_id AS sourceId
			FROM traffic_stats_v3
			WHERE granularity = '5m'
			ORDER BY source_id
		`
	);

	return rows.map((row) => row.sourceId);
}

export async function listDatasetSourceDefinitions(
	datasetId: string,
	platform?: App.Platform
): Promise<SourceDefinition[]> {
	const db = await getDatasetDb(datasetId, platform);
	const sourceIds = await listDatasetSources(datasetId, platform);
	const configured = await listConfiguredSourceDefinitions(db, datasetId);
	if (configured.length > 0) {
		return mergeSourceDefinitions(sourceIds, configured);
	}

	return inferSourceDefinitions(db, sourceIds);
}

async function listConfiguredSourceDefinitions(
	db: ReadonlyDatasetDb,
	datasetId: string
): Promise<SourceDefinition[]> {
	let rows: { sourceId: string; memberId: string }[];
	try {
		rows = await db.all<{ sourceId: string; memberId: string }>(
			`
				SELECT source_id AS sourceId, member_id AS memberId
				FROM source_members
				WHERE dataset_id = ?
				ORDER BY source_id, member_id
			`,
			[datasetId]
		);
	} catch {
		return [];
	}

	return groupSourceMemberRows(rows);
}

async function inferSourceDefinitions(
	db: ReadonlyDatasetDb,
	sourceIds: string[]
): Promise<SourceDefinition[]> {
	let rows: { sourceId: string; inputLocator: string }[];
	try {
		rows = await db.all<{ sourceId: string; inputLocator: string }>(
			`
				SELECT DISTINCT source_id AS sourceId, input_locator AS inputLocator
				FROM processed_inputs_v2
				WHERE input_kind = 'nfcapd'
					AND status = 'processed'
			`
		);
	} catch {
		rows = [];
	}

	const membersBySource = new Map<string, Set<string>>();
	for (const row of rows) {
		const memberId = inferMemberIdFromInputLocator(row.inputLocator);
		if (!memberId) {
			continue;
		}
		const members = membersBySource.get(row.sourceId) ?? new Set<string>();
		members.add(memberId);
		membersBySource.set(row.sourceId, members);
	}

	return sourceIds.map((sourceId) => ({
		sourceId,
		members: [...(membersBySource.get(sourceId) ?? new Set([sourceId]))].sort()
	}));
}

function groupSourceMemberRows(rows: { sourceId: string; memberId: string }[]): SourceDefinition[] {
	const membersBySource = new Map<string, Set<string>>();
	for (const row of rows) {
		const members = membersBySource.get(row.sourceId) ?? new Set<string>();
		members.add(row.memberId);
		membersBySource.set(row.sourceId, members);
	}

	return [...membersBySource]
		.map(([sourceId, members]) => ({ sourceId, members: [...members].sort() }))
		.sort((left, right) => left.sourceId.localeCompare(right.sourceId));
}

function mergeSourceDefinitions(
	sourceIds: string[],
	definitions: SourceDefinition[]
): SourceDefinition[] {
	const definitionsBySource = new Map(
		definitions.map((definition) => [definition.sourceId, definition])
	);
	for (const sourceId of sourceIds) {
		if (!definitionsBySource.has(sourceId)) {
			definitionsBySource.set(sourceId, { sourceId, members: [sourceId] });
		}
	}
	return [...definitionsBySource.values()].sort((left, right) =>
		left.sourceId.localeCompare(right.sourceId)
	);
}

function inferMemberIdFromInputLocator(inputLocator: string): string | null {
	if (inputLocator.startsWith('gap://')) {
		return null;
	}

	const parts = inputLocator.split('/').filter(Boolean);
	const filename = parts.at(-1) ?? '';
	if (!filename.startsWith('nfcapd.') || parts.length < 5) {
		return null;
	}

	return parts.at(-5) ?? null;
}

export async function listDatasetSummaries(platform?: App.Platform): Promise<DatasetSummary[]> {
	const defaultDatasetId = await getDefaultDatasetId(platform);
	const datasets = await listDatasetRows(platform);

	return Promise.all(
		datasets.map(async (dataset) => {
			const db = await getDatasetDb(dataset.id, platform);
			const sourceRows = await db.all<{ sourceCount: number }>(
				"SELECT COUNT(DISTINCT source_id) AS sourceCount FROM traffic_stats_v3 WHERE granularity = '5m'"
			);
			const sourceCount = sourceRows[0]?.sourceCount ?? 0;

			return {
				datasetId: dataset.id,
				label: dataset.label,
				defaultStartDate: dataset.defaultStartDate,
				discoveryMode: dataset.discoveryMode,
				sourceCount,
				isDefault: dataset.id === defaultDatasetId
			};
		})
	);
}

export async function getRequestedDataset(url: URL, platform?: App.Platform): Promise<string> {
	const dataset = url.searchParams.get('dataset')?.trim() || (await getDefaultDatasetId(platform));
	await getDatasetConfig(dataset, platform);
	return dataset;
}
