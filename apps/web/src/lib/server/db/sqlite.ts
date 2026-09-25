import {
	makePrepared,
	type DatabaseDriver,
	type DatasetDbLease,
	type DatasetRow,
	type QueryParam,
	type ReadonlyDatasetDb,
	type SourceMetadata
} from './driver.ts';
import { discoverLocalSqlitePaths } from './local-files.ts';
import { readAlertsFeed } from './sqlite-alerts.ts';

type LocalDatasetRow = DatasetRow & {
	dbPath: string;
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

type LocalDbCacheEntry = {
	db: ReadonlyDatasetDb;
	revision: LocalDbRevision;
	datasetRows?: LocalDatasetRow[];
	sourceMetadata: Map<string, SourceMetadata>;
	activeLeases: number;
	retired: boolean;
	close(): void;
};

const localDbCache = new Map<string, LocalDbCacheEntry>();
const localDbRefreshes = new Map<string, Promise<LocalDbCacheEntry>>();
// Local products are normally published with an atomic rename. The path cache is always validated
// against the file revision before reuse, so replacement or an in-place metadata write clears it.
const localDatasetPaths = new Map<string, string>();

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

async function openLocalClient(dbPath: string): Promise<SqliteClient> {
	const betterSqlite3 = await import(/* @vite-ignore */ 'better-sqlite3');
	const sqlite = new betterSqlite3.default(dbPath, { readonly: true, fileMustExist: true });
	sqlite.pragma('query_only = ON');
	sqlite.pragma('busy_timeout = 60000');
	return sqlite as SqliteClient;
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

async function acquireDatasetDb(datasetId: string): Promise<DatasetDbLease> {
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
					sourceMetadata: localEntry.sourceMetadata,
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

export default {
	listDatasetRows: listLocalDatasetRows,
	getDatasetRow: getLocalDatasetRow,
	acquireDatasetDb,
	readAlertsFeed
} satisfies DatabaseDriver;
