import { env as privateEnv } from '$env/dynamic/private';
import type Database from 'better-sqlite3';
import type {
	AlertFeedAlert,
	AlertFeedWindow,
	AlertsFeedResponse,
	AlertTail
} from '$lib/types/types';

type QueryParam = string | number | boolean | null | Uint8Array;

type SqliteClient = Database.Database;

type LocalDbIdentity = {
	device: number;
	inode: number;
};

type LocalDbCacheEntry = {
	db: SqliteClient;
	identity: LocalDbIdentity;
};

type FeedMetaRow = {
	key: string;
	value: string;
};

type WindowRow = {
	windowStart: number;
	windowEnd: number;
	addressCount: number;
	alertCount: number;
};

type LatestWindowRow = {
	windowStart: number;
	processedAt: number;
};

const DEFAULT_LIMIT_WINDOWS = 24;
const MAX_LIMIT_WINDOWS = 288;
const REQUIRED_TABLES = ['alerts', 'feed_meta', 'windows'] as const;
const REQUIRED_META_KEYS = [
	'schema_version',
	'dataset_id',
	'threshold_high',
	'threshold_low',
	'max_per_tail'
] as const;

const localDbCache = new Map<string, LocalDbCacheEntry>();

export type AlertsFeedOptions = {
	platform?: App.Platform;
	tail?: AlertTail;
	limitWindows?: number;
	before?: number;
};

function absentFeed(): AlertsFeedResponse {
	return { feed: { present: false }, windows: [] };
}

function getEnv(name: string): string | undefined {
	return globalThis.process?.env?.[name]?.trim() || privateEnv[name]?.trim() || undefined;
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

async function openReadonlyClient(dbPath: string): Promise<SqliteClient> {
	const betterSqlite3 = await import(/* @vite-ignore */ 'better-sqlite3');
	const sqlite = new betterSqlite3.default(dbPath, { readonly: true, fileMustExist: true });
	sqlite.pragma('query_only = ON');
	sqlite.pragma('busy_timeout = 60000');
	return sqlite;
}

async function resolveDatasetDirectory(datasetId: string): Promise<string | null> {
	const path = await import('node:path');
	for (const dbPath of await discoverLocalSqlitePaths()) {
		if (dbPath === ':memory:') {
			continue;
		}

		let db: SqliteClient | undefined;
		try {
			db = await openReadonlyClient(dbPath);
			const row = db.prepare('SELECT id FROM datasets WHERE id = ? LIMIT 1').get(datasetId) as
				| { id: string }
				| undefined;
			if (row) {
				return path.dirname(dbPath);
			}
		} catch {
			// A broken candidate cannot contain a usable local dataset.
		} finally {
			db?.close();
		}
	}

	return null;
}

async function localDbIdentity(dbPath: string): Promise<LocalDbIdentity> {
	const fs = await import('node:fs/promises');
	const stat = await fs.stat(dbPath);
	return { device: stat.dev, inode: stat.ino };
}

function sameLocalDbIdentity(left: LocalDbIdentity, right: LocalDbIdentity): boolean {
	return left.device === right.device && left.inode === right.inode;
}

function evictLocalDb(dbPath: string): void {
	const existing = localDbCache.get(dbPath);
	if (!existing) {
		return;
	}

	localDbCache.delete(dbPath);
	existing.db.close();
}

async function createLocalDb(dbPath: string): Promise<LocalDbCacheEntry> {
	for (let attempt = 0; attempt < 3; attempt += 1) {
		const identityBeforeOpen = await localDbIdentity(dbPath);
		const db = await openReadonlyClient(dbPath);
		try {
			const identityAfterOpen = await localDbIdentity(dbPath);
			if (sameLocalDbIdentity(identityBeforeOpen, identityAfterOpen)) {
				return { db, identity: identityAfterOpen };
			}
		} catch (error) {
			db.close();
			throw error;
		}
		db.close();
	}

	throw new Error(`Local alerts database kept changing while opening: ${dbPath}`);
}

async function getLocalDb(dbPath: string): Promise<SqliteClient> {
	try {
		const identity = await localDbIdentity(dbPath);
		const existing = localDbCache.get(dbPath);
		if (existing && sameLocalDbIdentity(existing.identity, identity)) {
			return existing.db;
		}

		evictLocalDb(dbPath);
		const entry = await createLocalDb(dbPath);
		localDbCache.set(dbPath, entry);
		return entry.db;
	} catch (error) {
		evictLocalDb(dbPath);
		throw error;
	}
}

function clampLimitWindows(limitWindows: number | undefined): number {
	if (limitWindows === undefined || !Number.isFinite(limitWindows)) {
		return DEFAULT_LIMIT_WINDOWS;
	}
	return Math.min(MAX_LIMIT_WINDOWS, Math.max(1, Math.trunc(limitWindows)));
}

function readFeedMetadata(
	db: SqliteClient,
	datasetId: string
): { thresholdHigh: number; thresholdLow: number } {
	const tables = db
		.prepare(
			`SELECT name FROM sqlite_master WHERE type = 'table' AND name IN (${REQUIRED_TABLES.map(() => '?').join(', ')})`
		)
		.all(...REQUIRED_TABLES) as { name: string }[];
	if (new Set(tables.map((row) => row.name)).size !== REQUIRED_TABLES.length) {
		throw new Error('Alerts database is missing required tables');
	}
	db.prepare(
		`SELECT
			window_start,
			window_end,
			member_files,
			address_count,
			alert_count,
			alpha_min,
			alpha_max,
			alpha_median,
			processed_at
		FROM windows
		LIMIT 0`
	).all();
	db.prepare(
		`SELECT window_start, address, alpha, tail, rank, r2, prefix_levels
		FROM alerts
		LIMIT 0`
	).all();

	const rows = db
		.prepare(
			`SELECT key, value FROM feed_meta WHERE key IN (${REQUIRED_META_KEYS.map(() => '?').join(', ')})`
		)
		.all(...REQUIRED_META_KEYS) as FeedMetaRow[];
	const metadata = new Map(rows.map((row) => [row.key, row.value]));
	const thresholdHigh = Number(metadata.get('threshold_high'));
	const thresholdLow = Number(metadata.get('threshold_low'));
	const maxPerTail = Number(metadata.get('max_per_tail'));

	if (
		metadata.get('schema_version') !== '1' ||
		metadata.get('dataset_id') !== datasetId ||
		!Number.isFinite(thresholdHigh) ||
		!Number.isFinite(thresholdLow) ||
		!Number.isSafeInteger(maxPerTail) ||
		maxPerTail < 1
	) {
		throw new Error('Alerts database metadata is invalid');
	}

	return { thresholdHigh, thresholdLow };
}

function readAlertsForWindow(
	db: SqliteClient,
	windowStart: number,
	tail: AlertTail | undefined
): AlertFeedAlert[] {
	const whereTail = tail ? 'AND tail = ?' : '';
	const params: QueryParam[] = tail ? [windowStart, tail] : [windowStart];
	return db
		.prepare(
			`
				SELECT address, alpha, tail, rank, r2
				FROM alerts
				WHERE window_start = ?
				${whereTail}
				ORDER BY CASE tail WHEN 'high' THEN 0 ELSE 1 END, rank ASC
			`
		)
		.all(...params) as AlertFeedAlert[];
}

function readWindows(
	db: SqliteClient,
	options: Pick<AlertsFeedOptions, 'tail' | 'limitWindows' | 'before'>
): AlertFeedWindow[] {
	const limitWindows = clampLimitWindows(options.limitWindows);
	const beforeClause = options.before === undefined ? '' : 'WHERE window_start < ?';
	const params: QueryParam[] =
		options.before === undefined ? [limitWindows] : [options.before, limitWindows];
	const rows = db
		.prepare(
			`
				SELECT
					window_start AS windowStart,
					window_end AS windowEnd,
					address_count AS addressCount,
					alert_count AS alertCount
				FROM windows
				${beforeClause}
				ORDER BY window_start DESC
				LIMIT ?
			`
		)
		.all(...params) as WindowRow[];

	return rows.map((row) => ({
		...row,
		alerts: readAlertsForWindow(db, row.windowStart, options.tail)
	}));
}

export async function getAlertsFeedForDataset(
	datasetId: string,
	options: AlertsFeedOptions = {}
): Promise<AlertsFeedResponse> {
	if (shouldUseD1(options.platform)) {
		return absentFeed();
	}

	let alertsDbPath: string | undefined;
	try {
		const datasetDirectory = await resolveDatasetDirectory(datasetId);
		if (!datasetDirectory) {
			return absentFeed();
		}

		const path = await import('node:path');
		alertsDbPath = path.join(datasetDirectory, 'alerts.sqlite');
		const db = await getLocalDb(alertsDbPath);
		const metadata = readFeedMetadata(db, datasetId);
		const latestWindow = db
			.prepare(
				`
					SELECT window_start AS windowStart, processed_at AS processedAt
					FROM windows
					ORDER BY window_start DESC
					LIMIT 1
				`
			)
			.get() as LatestWindowRow | undefined;

		return {
			feed: {
				present: true,
				latestWindowStart: latestWindow?.windowStart ?? null,
				latestProcessedAt: latestWindow?.processedAt ?? null,
				thresholds: { high: metadata.thresholdHigh, low: metadata.thresholdLow }
			},
			windows: readWindows(db, options)
		};
	} catch {
		if (alertsDbPath) {
			evictLocalDb(alertsDbPath);
		}
		return absentFeed();
	}
}
