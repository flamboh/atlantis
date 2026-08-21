import { error } from '@sveltejs/kit';
import type { PageLoad } from './$types';
import { loadDatasetSummariesFromFetch } from '$lib/datasets';

export const load: PageLoad = async ({ params, fetch }) => {
	const { dataset } = params;

	if (!dataset || dataset.trim().length === 0) {
		throw error(400, 'Dataset parameter is required');
	}

	const datasets = await loadDatasetSummariesFromFetch(fetch);
	const selectedDataset = datasets.find((entry) => entry.datasetId === dataset);
	if (!selectedDataset) {
		throw error(404, `Unknown dataset '${dataset}'`);
	}

	const routersResponse = await fetch(`/api/routers?dataset=${encodeURIComponent(dataset)}`);
	const routersPayload = await routersResponse.json();
	if (!routersResponse.ok) {
		throw error(
			routersResponse.status,
			typeof routersPayload?.error === 'string'
				? routersPayload.error
				: 'Failed to load source metadata'
		);
	}

	if (
		!Array.isArray(routersPayload) ||
		!routersPayload.every((router): router is string => typeof router === 'string')
	) {
		throw error(500, 'Invalid source metadata response');
	}

	return {
		datasetId: selectedDataset.datasetId,
		title: selectedDataset.label,
		defaultStartDate: selectedDataset.defaultStartDate,
		routers: routersPayload
	};
};
