import { error } from '@sveltejs/kit';
import type { PageLoad } from './$types';
import { loadDatasetSummariesFromFetch, resolveDefaultDatasetId } from '#lib/datasets.ts';

export const load: PageLoad = async ({ fetch, url }) => {
	try {
		const datasets = await loadDatasetSummariesFromFetch(fetch);
		const requestedDataset = url.searchParams.get('dataset')?.trim() || '';
		const selectedDataset =
			requestedDataset && datasets.some((dataset) => dataset.datasetId === requestedDataset)
				? requestedDataset
				: resolveDefaultDatasetId(datasets);

		return {
			datasets,
			selectedDataset
		};
	} catch (err) {
		throw error(500, err instanceof Error ? err.message : 'Failed to load datasets');
	}
};
