import { z } from 'zod/mini';
import type { DatasetSummariesResponse, DatasetSummary } from '#lib/types/types.ts';

export type { DatasetSummariesResponse, DatasetSummary } from '#lib/types/types.ts';

const datasetSummarySchema = z.object({
	datasetId: z.string().check(z.minLength(1)),
	label: z.string().check(z.minLength(1)),
	defaultStartDate: z.iso.date(),
	discoveryMode: z.string().check(z.minLength(1)),
	hasLocality: z.boolean(),
	isDefault: z.boolean()
});

const datasetSummariesResponseSchema = z.object({
	data: z.nullable(z.array(datasetSummarySchema)),
	error: z.nullable(z.string())
});

let cachedDatasetSummaries: DatasetSummary[] | null = null;
let pendingDatasetSummariesRequest: Promise<DatasetSummary[]> | null = null;

export function getCachedDatasetSummaries(): DatasetSummary[] | null {
	return cachedDatasetSummaries;
}

export function parseDatasetSummariesResponse(payload: unknown): DatasetSummariesResponse {
	return datasetSummariesResponseSchema.parse(payload);
}

export function cacheDatasetSummaries(datasets: DatasetSummary[]): DatasetSummary[] {
	cachedDatasetSummaries = datasets;
	return datasets;
}

export function resolveDefaultDatasetId(datasets: DatasetSummary[]): string {
	return datasets.find((dataset) => dataset.isDefault)?.datasetId ?? datasets[0]?.datasetId ?? '';
}

export async function loadDatasetSummariesFromFetch(
	fetchFn: typeof fetch
): Promise<DatasetSummary[]> {
	const response = await fetchFn('/api/datasets');
	const payload = parseDatasetSummariesResponse(await response.json());
	if (!response.ok || payload.error || payload.data === null) {
		throw new Error(payload.error || `Failed to load datasets: ${response.statusText}`);
	}
	return cacheDatasetSummaries(payload.data);
}

export async function loadDatasetSummaries(): Promise<DatasetSummary[]> {
	if (cachedDatasetSummaries) {
		return cachedDatasetSummaries;
	}

	if (pendingDatasetSummariesRequest) {
		return pendingDatasetSummariesRequest;
	}

	pendingDatasetSummariesRequest = loadDatasetSummariesFromFetch(fetch).finally(() => {
		pendingDatasetSummariesRequest = null;
	});

	return pendingDatasetSummariesRequest;
}
