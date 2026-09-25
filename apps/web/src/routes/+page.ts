import { error } from '@sveltejs/kit';
import type { PageLoad } from './$types';
import { loadDatasetSummariesFromFetch } from '#lib/datasets.ts';

export const load: PageLoad = async ({ fetch }) => {
	try {
		return {
			datasets: await loadDatasetSummariesFromFetch(fetch)
		};
	} catch (err) {
		throw error(500, err instanceof Error ? err.message : 'Failed to load datasets');
	}
};
