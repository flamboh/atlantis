import { DEFAULT_DATASET } from '$app/env/private';
import database from '#db';
import type {
	DatasetDbLease,
	DatasetRow,
	ReadonlyDatasetDb,
	SourceDefinition
} from '#lib/server/db/driver.ts';
import type { DatasetSummary } from '#lib/types/types.ts';

export type {
	PreparedStatement,
	ReadonlyDatasetDb,
	SourceDefinition
} from '#lib/server/db/driver.ts';

export async function listDatasets(): Promise<DatasetRow[]> {
	return database.listDatasetRows();
}

export async function getDefaultDatasetId(): Promise<string> {
	const datasets = await database.listDatasetRows();
	return getDefaultDatasetIdFromRows(datasets);
}

function getDefaultDatasetIdFromRows(datasets: DatasetRow[]): string {
	const configured = DEFAULT_DATASET;
	if (configured && datasets.some((dataset) => dataset.id === configured)) {
		return configured;
	}

	const firstDataset = datasets[0];
	if (!firstDataset) {
		throw new Error('No datasets configured');
	}

	return firstDataset.id;
}

export async function getDatasetConfig(datasetId: string): Promise<DatasetRow> {
	return database.getDatasetRow(datasetId);
}

export async function getDatasetLabel(datasetId: string): Promise<string> {
	const dataset = await getDatasetConfig(datasetId);
	return dataset.label.trim() || dataset.id;
}

export type DatasetDbSession = {
	db: ReadonlyDatasetDb;
	listSources(): Promise<string[]>;
	listSourceDefinitions(): Promise<SourceDefinition[]>;
};

export async function withDatasetDb<T>(
	datasetId: string,
	run: (session: DatasetDbSession) => Promise<T> | T
): Promise<T> {
	const lease = await database.acquireDatasetDb(datasetId);
	try {
		return await run({
			db: lease.db,
			listSources: () => listDatasetSourcesFromLease(datasetId, lease),
			listSourceDefinitions: () => listDatasetSourceDefinitionsFromLease(datasetId, lease)
		});
	} finally {
		lease.release();
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

export async function listDatasetSources(datasetId: string): Promise<string[]> {
	return withDatasetDb(datasetId, ({ listSources }) => listSources());
}

async function listDatasetSourcesFromLease(
	datasetId: string,
	{ db, sourceMetadata }: DatasetDbLease
): Promise<string[]> {
	const cached = sourceMetadata?.get(datasetId);
	if (cached?.sourceIds) {
		return [...cached.sourceIds];
	}

	const configured = await listConfiguredSourceDefinitions(db, datasetId);
	const sourceIds =
		configured.length > 0
			? configured.map((definition) => definition.sourceId)
			: await listDatasetSourcesFromDb(db);
	sourceMetadata?.set(datasetId, {
		sourceIds,
		definitions: configured.length > 0 ? configured : undefined
	});
	return [...sourceIds];
}

export async function listDatasetSourceDefinitions(datasetId: string): Promise<SourceDefinition[]> {
	return withDatasetDb(datasetId, ({ listSourceDefinitions }) => listSourceDefinitions());
}

async function listDatasetSourceDefinitionsFromLease(
	datasetId: string,
	{ db, sourceMetadata }: DatasetDbLease
): Promise<SourceDefinition[]> {
	const cached = sourceMetadata?.get(datasetId);
	if (cached?.definitions) {
		return copySourceDefinitions(cached.definitions);
	}

	const configured = await listConfiguredSourceDefinitions(db, datasetId);
	if (configured.length > 0) {
		sourceMetadata?.set(datasetId, {
			sourceIds: configured.map((definition) => definition.sourceId),
			definitions: configured
		});
		return copySourceDefinitions(configured);
	}

	const sourceIds = cached?.sourceIds ?? (await listDatasetSourcesFromDb(db));
	const definitions = await inferSourceDefinitions(db, sourceIds);
	sourceMetadata?.set(datasetId, { sourceIds, definitions });
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

export async function listDatasetSummaries(): Promise<DatasetSummary[]> {
	// Zero datasets is a valid state (fresh checkout before the pipeline runs);
	// the dashboard shows setup guidance instead of an error.
	const datasets = await database.listDatasetRows();
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

export async function getRequestedDataset(url: URL): Promise<string> {
	const requested = url.searchParams.get('dataset')?.trim();
	if (!requested) {
		return getDefaultDatasetId();
	}

	await getDatasetConfig(requested);
	return requested;
}
