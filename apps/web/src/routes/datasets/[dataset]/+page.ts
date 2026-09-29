import { error } from '@sveltejs/kit';
import type { PageLoad } from './$types';
import { loadDatasetSummariesFromFetch } from '#lib/datasets.ts';

export const load: PageLoad = async ({ params, fetch }) => {
	const { dataset } = params;

	if (!dataset || dataset.trim().length === 0) {
		throw error(400, 'Dataset parameter is required');
	}

	const datasetsPromise = loadDatasetSummariesFromFetch(fetch);
	const routersResponsePromise = fetch(`/api/routers?dataset=${encodeURIComponent(dataset)}`);
	const maadStatusResponsePromise = fetch(
		`/api/netflow/maad-status?dataset=${encodeURIComponent(dataset)}`
	);
	const [datasets, routersResponse, maadStatusResponse] = await Promise.all([
		datasetsPromise,
		routersResponsePromise,
		maadStatusResponsePromise
	]);
	const selectedDataset = datasets.find((entry) => entry.datasetId === dataset);
	if (!selectedDataset) {
		throw error(404, `Unknown dataset '${dataset}'`);
	}

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

	const maadStatusPayload = await maadStatusResponse.json();
	if (!maadStatusResponse.ok || typeof maadStatusPayload?.computed !== 'boolean') {
		throw error(
			maadStatusResponse.ok ? 500 : maadStatusResponse.status,
			typeof maadStatusPayload?.error === 'string'
				? maadStatusPayload.error
				: 'Invalid MAAD status response'
		);
	}

	return {
		datasetId: selectedDataset.datasetId,
		title: selectedDataset.label,
		defaultStartDate: selectedDataset.defaultStartDate,
		hasLocality: selectedDataset.hasLocality,
		maadComputed: maadStatusPayload.computed,
		routers: routersPayload
	};
};
