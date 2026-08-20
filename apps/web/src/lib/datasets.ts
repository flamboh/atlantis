import { z } from 'zod';
import type { DatasetSummariesResponse, DatasetSummary } from '$lib/types/types';

export type { DatasetSummariesResponse, DatasetSummary } from '$lib/types/types';

const datasetSummarySchema = z.object({
	datasetId: z.string().min(1),
	label: z.string().min(1),
	defaultStartDate: z.iso.date(),
	discoveryMode: z.string().min(1),
	isDefault: z.boolean()
});

const datasetSummariesResponseSchema = z.object({
	data: z.array(datasetSummarySchema).nullable(),
	error: z.string().nullable()
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
