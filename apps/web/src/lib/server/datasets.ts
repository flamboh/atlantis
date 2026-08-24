import type { D1Database } from '@cloudflare/workers-types';
import { env as privateEnv } from '$env/dynamic/private';
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
	close(): void;
	prepare(sql: string): {
		get(...params: QueryParam[]): unknown;
		all(...params: QueryParam[]): unknown[];
	};
};

type LocalFileRevision = {
	device: number;
	inode: number;
	size: number;
	modifiedMs: number;
};

type LocalDbIdentity = Pick<LocalFileRevision, 'device' | 'inode'>;

type LocalDbRevision = LocalFileRevision & {
	wal: LocalFileRevision | null;
};

type LocalSourceMetadata = {
	sourceIds?: string[];
	definitions?: SourceDefinition[];
};

type LocalDbCacheEntry = {
	db: ReadonlyDatasetDb;
	revision: LocalDbRevision;
	datasetRows?: LocalDatasetRow[];
	sourceMetadata: Map<string, LocalSourceMetadata>;
	activeLeases: number;
	retired: boolean;
	close(): void;
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

const localDbCache = new Map<string, LocalDbCacheEntry>();
const localDbRefreshes = new Map<string, Promise<LocalDbCacheEntry>>();
const d1DbCache = new WeakMap<D1Database, ReadonlyDatasetDb>();
// Local products are normally published with an atomic rename. The path cache is always validated
// against the file revision before reuse, so replacement or an in-place metadata write clears it.
const localDatasetPaths = new Map<string, string>();

function getEnv(name: string): string | undefined {
	return globalThis.process?.env?.[name]?.trim() || privateEnv[name]?.trim() || undefined;
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
	const cached = d1DbCache.get(d1);
	if (cached) {
		return cached;
	}

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

	d1DbCache.set(d1, db);
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

	return [...dbPaths].sort();
}

async function openLocalClient(dbPath: string): Promise<SqliteClient> {
	const [{ drizzle }, betterSqlite3, schema] = await Promise.all([
		import(/* @vite-ignore */ 'drizzle-orm/better-sqlite3'),
		import(/* @vite-ignore */ 'better-sqlite3'),
		import('$lib/server/db/schema')
	]);
	const sqlite = new betterSqlite3.default(dbPath, { readonly: true, fileMustExist: true });
	sqlite.pragma('query_only = ON');
	sqlite.pragma('busy_timeout = 60000');
	const drizzleDb = drizzle(sqlite, { schema });
	return drizzleDb.$client as SqliteClient;
}

async function localDbRevision(dbPath: string): Promise<LocalDbRevision> {
	const fs = await import('node:fs/promises');
	const [stat, wal] = await Promise.all([
		fs.stat(dbPath),
		fs.stat(`${dbPath}-wal`).catch((error: unknown) => {
			if (
				typeof error === 'object' &&
				error !== null &&
				'code' in error &&
				error.code === 'ENOENT'
			) {
				return null;
			}
			throw error;
		})
	]);
	const revision = (file: typeof stat): LocalFileRevision => ({
		device: file.dev,
		inode: file.ino,
		size: file.size,
		modifiedMs: file.mtimeMs
	});
	return {
		...revision(stat),
		wal: wal ? revision(wal) : null
	};
}

function sameLocalDbIdentity(left: LocalDbIdentity, right: LocalDbIdentity): boolean {
	return left.device === right.device && left.inode === right.inode;
}

function sameLocalFileRevision(left: LocalFileRevision, right: LocalFileRevision): boolean {
	return (
		left.device === right.device &&
		left.inode === right.inode &&
		left.size === right.size &&
		left.modifiedMs === right.modifiedMs
	);
}

function sameLocalDbRevision(left: LocalDbRevision, right: LocalDbRevision): boolean {
	return (
		sameLocalFileRevision(left, right) &&
		(left.wal === null
			? right.wal === null
			: right.wal !== null && sameLocalFileRevision(left.wal, right.wal))
	);
}

function evictLocalDatasetPaths(dbPath: string): void {
	for (const [datasetId, cachedPath] of localDatasetPaths) {
		if (cachedPath === dbPath) {
			localDatasetPaths.delete(datasetId);
		}
	}
}

async function createLocalDb(dbPath: string): Promise<LocalDbCacheEntry> {
	for (let attempt = 0; attempt < 3; attempt += 1) {
		const revisionBeforeOpen = await localDbRevision(dbPath);
		const client = await openLocalClient(dbPath);
		const revisionAfterOpen = await localDbRevision(dbPath);
		if (sameLocalDbIdentity(revisionBeforeOpen, revisionAfterOpen)) {
			let closed = false;
			return {
				db: createReadonlyDb(client),
				revision: revisionAfterOpen,
				sourceMetadata: new Map(),
				activeLeases: 0,
				retired: false,
				close: () => {
					if (!closed) {
						closed = true;
						client.close();
					}
				}
			};
		}
		client.close();
	}

	throw new Error(`Local SQLite database kept changing while opening: ${dbPath}`);
}

function retireLocalDbEntry(entry: LocalDbCacheEntry): void {
	entry.retired = true;
	if (entry.activeLeases === 0) {
		entry.close();
	}
}

function releaseLocalDbEntry(entry: LocalDbCacheEntry): void {
	if (entry.activeLeases === 0) {
		throw new Error('Local SQLite database lease released more than once');
	}
	entry.activeLeases -= 1;
	if (entry.retired && entry.activeLeases === 0) {
		entry.close();
	}
}

async function getLocalDbEntry(dbPath: string): Promise<LocalDbCacheEntry> {
	const pendingRefresh = localDbRefreshes.get(dbPath);
	if (pendingRefresh) {
		await pendingRefresh;
	}

	const revision = await localDbRevision(dbPath);
	const existing = localDbCache.get(dbPath);
	if (existing && !existing.retired && sameLocalDbIdentity(existing.revision, revision)) {
		if (!sameLocalDbRevision(existing.revision, revision)) {
			existing.revision = revision;
			existing.datasetRows = undefined;
			existing.sourceMetadata.clear();
			evictLocalDatasetPaths(dbPath);
		}
		return existing;
	}

	const concurrentRefresh = localDbRefreshes.get(dbPath);
	if (concurrentRefresh) {
		await concurrentRefresh;
		return getLocalDbEntry(dbPath);
	}

	const refresh = (async () => {
		const stale = localDbCache.get(dbPath);
		if (stale) {
			localDbCache.delete(dbPath);
			retireLocalDbEntry(stale);
		}
		evictLocalDatasetPaths(dbPath);

		const entry = await createLocalDb(dbPath);
		localDbCache.set(dbPath, entry);
		return entry;
	})();
	localDbRefreshes.set(dbPath, refresh);
	try {
		return await refresh;
	} finally {
		if (localDbRefreshes.get(dbPath) === refresh) {
			localDbRefreshes.delete(dbPath);
		}
	}
}

async function acquireLocalDbEntry(dbPath: string): Promise<LocalDbCacheEntry> {
	for (;;) {
		const entry = await getLocalDbEntry(dbPath);
		if (!entry.retired) {
			entry.activeLeases += 1;
			return entry;
		}
	}
}

async function readDatasetRowsFromEntry(
	dbPath: string,
	entry: LocalDbCacheEntry
): Promise<LocalDatasetRow[]> {
	if (entry.datasetRows) {
		return entry.datasetRows.map((row) => ({ ...row }));
	}

	const rows = await entry.db.all<DatasetRow>(
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
	// Backups and obsolete products can remain under data/, but the current dashboard requires
	// explicit coverage and must not let an older database shadow a current product with the same ID.
	const schema = await entry.db.get<{ hasCoverage: number }>(
		`SELECT EXISTS(
			SELECT 1
			FROM sqlite_master
			WHERE type = 'table' AND name = 'bucket_coverage'
		) AS hasCoverage`
	);
	if (schema?.hasCoverage !== 1) {
		entry.datasetRows = [];
		return [];
	}

	entry.datasetRows = rows.map((row) => ({ ...row, dbPath }));
	return entry.datasetRows.map((row) => ({ ...row }));
}

async function readDatasetRowsFromDb(dbPath: string): Promise<LocalDatasetRow[]> {
	const entry = await acquireLocalDbEntry(dbPath);
	try {
		return await readDatasetRowsFromEntry(dbPath, entry);
	} finally {
		releaseLocalDbEntry(entry);
	}
}

async function listLocalDatasetRows(): Promise<LocalDatasetRow[]> {
	const dbPaths = await discoverLocalSqlitePaths();
	const rows = (await Promise.all(dbPaths.map(readDatasetRowsFromDb)))
		.flat()
		.sort((left, right) => left.sortOrder - right.sortOrder || left.id.localeCompare(right.id));

	localDatasetPaths.clear();
	for (const row of rows) {
		if (!localDatasetPaths.has(row.id)) {
			localDatasetPaths.set(row.id, row.dbPath);
		}
	}
	return rows;
}

async function getLocalDatasetRow(datasetId: string): Promise<LocalDatasetRow> {
	const cachedPath = localDatasetPaths.get(datasetId);
	if (cachedPath) {
		try {
			const cached = (await readDatasetRowsFromDb(cachedPath)).find(
				(dataset) => dataset.id === datasetId
			);
			if (cached) {
				localDatasetPaths.set(datasetId, cachedPath);
				return cached;
			}
		} catch {
			// A moved or replaced product is resolved through fresh discovery below.
		}
		localDatasetPaths.delete(datasetId);
	}

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

async function getD1DatasetRow(datasetId: string, platform: App.Platform): Promise<DatasetRow> {
	const db = createD1Db(platform.env.DB);
	const dataset = await db.get<DatasetRow>(
		`
			SELECT
				id,
				label,
				default_start_date AS defaultStartDate,
				discovery_mode AS discoveryMode,
				sort_order AS sortOrder
			FROM datasets
			WHERE id = ?
			LIMIT 1
		`,
		[datasetId]
	);
	if (dataset) {
		return dataset;
	}

	const available = (await listD1DatasetRows(platform)).map((item) => item.id).join(', ');
	throw new Error(`Unknown dataset '${datasetId}'. Available datasets: ${available}`);
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
	const datasets = await listDatasetRows(platform);
	return getDefaultDatasetIdFromRows(datasets);
}

function getDefaultDatasetIdFromRows(datasets: DatasetRow[]): string {
	const configured = getEnv('DEFAULT_DATASET');
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

	return getD1DatasetRow(datasetId, platform as App.Platform);
}

export async function getDatasetLabel(datasetId: string, platform?: App.Platform): Promise<string> {
	const dataset = await getDatasetConfig(datasetId, platform);
	return dataset.label.trim() || dataset.id;
}

type DatasetDbContext = {
	db: ReadonlyDatasetDb;
	localEntry?: LocalDbCacheEntry;
	release(): void;
};

async function acquireDatasetDbContext(
	datasetId: string,
	platform?: App.Platform
): Promise<DatasetDbContext> {
	if (shouldUseD1(platform)) {
		return { db: createD1Db((platform as App.Platform).env.DB), release: () => undefined };
	}

	for (let attempt = 0; attempt < 3; attempt += 1) {
		const dataset = await getLocalDatasetRow(datasetId);
		const localEntry = await acquireLocalDbEntry(dataset.dbPath);
		try {
			const currentDataset = (await readDatasetRowsFromEntry(dataset.dbPath, localEntry)).some(
				(row) => row.id === datasetId
			);
			if (currentDataset) {
				return {
					db: localEntry.db,
					localEntry,
					release: () => releaseLocalDbEntry(localEntry)
				};
			}
		} catch (error) {
			releaseLocalDbEntry(localEntry);
			throw error;
		}
		releaseLocalDbEntry(localEntry);
		localDatasetPaths.delete(datasetId);
	}

	throw new Error(`Dataset '${datasetId}' kept changing while opening its database`);
}

export type DatasetDbSession = {
	db: ReadonlyDatasetDb;
	listSources(): Promise<string[]>;
	listSourceDefinitions(): Promise<SourceDefinition[]>;
};

export async function withDatasetDb<T>(
	datasetId: string,
	platform: App.Platform | undefined,
	run: (session: DatasetDbSession) => Promise<T> | T
): Promise<T> {
	const context = await acquireDatasetDbContext(datasetId, platform);
	try {
		return await run({
			db: context.db,
			listSources: () => listDatasetSourcesFromContext(datasetId, context),
			listSourceDefinitions: () => listDatasetSourceDefinitionsFromContext(datasetId, context)
		});
	} finally {
		context.release();
	}
}

async function listDatasetSourcesFromDb(db: ReadonlyDatasetDb): Promise<string[]> {
	const rows = await db.all<{ sourceId: string }>(
		`
			SELECT DISTINCT source_id AS sourceId
			FROM traffic_stats
			WHERE granularity = '5m'
			ORDER BY source_id
		`
	);
	return rows.map((row) => row.sourceId);
}

function copySourceDefinitions(definitions: SourceDefinition[]): SourceDefinition[] {
	return definitions.map((definition) => ({
		sourceId: definition.sourceId,
		members: [...definition.members]
	}));
}

export async function listDatasetSources(
	datasetId: string,
	platform?: App.Platform
): Promise<string[]> {
	return withDatasetDb(datasetId, platform, ({ listSources }) => listSources());
}

async function listDatasetSourcesFromContext(
	datasetId: string,
	{ db, localEntry }: DatasetDbContext
): Promise<string[]> {
	const cached = localEntry?.sourceMetadata.get(datasetId);
	if (cached?.sourceIds) {
		return [...cached.sourceIds];
	}

	const configured = await listConfiguredSourceDefinitions(db, datasetId);
	const sourceIds =
		configured.length > 0
			? configured.map((definition) => definition.sourceId)
			: await listDatasetSourcesFromDb(db);
	if (localEntry) {
		localEntry.sourceMetadata.set(datasetId, {
			sourceIds,
			definitions: configured.length > 0 ? configured : undefined
		});
	}
	return [...sourceIds];
}

export async function listDatasetSourceDefinitions(
	datasetId: string,
	platform?: App.Platform
): Promise<SourceDefinition[]> {
	return withDatasetDb(datasetId, platform, ({ listSourceDefinitions }) => listSourceDefinitions());
}

async function listDatasetSourceDefinitionsFromContext(
	datasetId: string,
	{ db, localEntry }: DatasetDbContext
): Promise<SourceDefinition[]> {
	const cached = localEntry?.sourceMetadata.get(datasetId);
	if (cached?.definitions) {
		return copySourceDefinitions(cached.definitions);
	}

	const configured = await listConfiguredSourceDefinitions(db, datasetId);
	if (configured.length > 0) {
		if (localEntry) {
			localEntry.sourceMetadata.set(datasetId, {
				sourceIds: configured.map((definition) => definition.sourceId),
				definitions: configured
			});
		}
		return copySourceDefinitions(configured);
	}

	const sourceIds = cached?.sourceIds ?? (await listDatasetSourcesFromDb(db));
	const definitions = await inferSourceDefinitions(db, sourceIds);
	if (localEntry) {
		localEntry.sourceMetadata.set(datasetId, { sourceIds, definitions });
	}
	return copySourceDefinitions(definitions);
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
				FROM processed_inputs
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
	// Zero datasets is a valid state (fresh checkout before the pipeline runs);
	// the dashboard shows setup guidance instead of an error.
	const datasets = await listDatasetRows(platform);
	if (datasets.length === 0) {
		return [];
	}

	const defaultDatasetId = getDefaultDatasetIdFromRows(datasets);

	return datasets.map((dataset) => ({
		datasetId: dataset.id,
		label: dataset.label,
		defaultStartDate: dataset.defaultStartDate,
		discoveryMode: dataset.discoveryMode,
		isDefault: dataset.id === defaultDatasetId
	}));
}

export async function getRequestedDataset(url: URL, platform?: App.Platform): Promise<string> {
	const requested = url.searchParams.get('dataset')?.trim();
	if (!requested) {
		return getDefaultDatasetId(platform);
	}

	await getDatasetConfig(requested, platform);
	return requested;
}
