import { error } from '@sveltejs/kit';
import type { PageLoad } from './$types';
import { loadDatasetSummariesFromFetch } from '$lib/datasets';
import type { AlertsFeedResponse } from '$lib/types/types';

type ErrorResponse = {
	data: null;
	error: string;
};

export const load: PageLoad = async ({ fetch, params }) => {
	const datasets = await loadDatasetSummariesFromFetch(fetch);
	const selectedDataset = params.dataset;
	if (!datasets.some((dataset) => dataset.datasetId === selectedDataset)) {
		throw error(404, `Unknown dataset '${selectedDataset}'`);
	}

	try {
		const response = await fetch(
			`/api/alerts?dataset=${encodeURIComponent(selectedDataset)}&tail=high&horizon=24h&sort=extreme&limit=100`
		);
		const alerts = (await response.json()) as AlertsFeedResponse | ErrorResponse;
		if (!response.ok || 'error' in alerts) {
			throw new Error('error' in alerts ? alerts.error : 'Failed to load alerts feed');
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
