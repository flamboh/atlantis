import { error } from '@sveltejs/kit';
import type { PageLoad } from './$types';
import { loadDatasetSummariesFromFetch, resolveDefaultDatasetId } from '$lib/datasets';
import type { AlertsFeedResponse } from '$lib/types/types';

type ErrorResponse = {
	data: null;
	error: string;
};

export const load: PageLoad = async ({ fetch, url }) => {
	try {
		const datasets = await loadDatasetSummariesFromFetch(fetch);
		const requestedDataset = url.searchParams.get('dataset')?.trim() || '';
		const selectedDataset =
			requestedDataset && datasets.some((dataset) => dataset.datasetId === requestedDataset)
				? requestedDataset
				: resolveDefaultDatasetId(datasets);

		let alerts: AlertsFeedResponse = {
			feed: { present: false },
			horizonSeconds: 86_400,
			totalAddresses: 0,
			addresses: []
		};
		if (selectedDataset) {
			const response = await fetch(
				`/api/alerts?dataset=${encodeURIComponent(selectedDataset)}&horizon=24h&sort=extreme&limit=100`
			);
			const payload = (await response.json()) as AlertsFeedResponse | ErrorResponse;
			if (!response.ok || 'error' in payload) {
				throw new Error('error' in payload ? payload.error : 'Failed to load alerts feed');
			}
			alerts = payload;
		}

		return {
			datasets,
			selectedDataset,
			alerts
		};
	} catch (err) {
		throw error(500, err instanceof Error ? err.message : 'Failed to load alerts feed');
	}
};
