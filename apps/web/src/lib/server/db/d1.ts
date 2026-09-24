import type { D1Database } from '@cloudflare/workers-types';
import {
	makePrepared,
	type DatabaseDriver,
	type DatasetRow,
	type QueryParam,
	type ReadonlyDatasetDb
} from './driver.ts';

const d1DbCache = new WeakMap<D1Database, ReadonlyDatasetDb>();

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

async function bindingDb(): Promise<ReadonlyDatasetDb> {
	const { env } = await import('cloudflare:workers');
	return createD1Db(env.DB);
}

async function listDatasetRows(): Promise<DatasetRow[]> {
	const db = await bindingDb();
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

async function getDatasetRow(datasetId: string): Promise<DatasetRow> {
	const db = await bindingDb();
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

	const available = (await listDatasetRows()).map((item) => item.id).join(', ');
	throw new Error(`Unknown dataset '${datasetId}'. Available datasets: ${available}`);
}

export default {
	listDatasetRows,
	getDatasetRow,
	async acquireDatasetDb() {
		return { db: await bindingDb(), release: () => undefined };
	},
	async readAlertsFeed() {
		return null;
	}
} satisfies DatabaseDriver;
