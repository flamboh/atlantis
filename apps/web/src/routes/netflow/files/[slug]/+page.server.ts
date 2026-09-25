import { error } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';
import { loadDatasetSummariesFromFetch, resolveDefaultDatasetId } from '$lib/datasets';
import { parseFlowDirectionParams } from '$lib/server/netflow-v3';

export const load: PageServerLoad = async ({ params, url, fetch }) => {
	const { slug } = params;
	let dataset = url.searchParams.get('dataset')?.trim() || '';
	const flowDirection = parseFlowDirectionParams(url);

	if ('error' in flowDirection) {
		throw error(flowDirection.status, flowDirection.error);
	}

	if (!dataset) {
		const datasets = await loadDatasetSummariesFromFetch(fetch);
		dataset = resolveDefaultDatasetId(datasets);
		if (!dataset) {
			throw error(404, 'No datasets are configured');
		}
	}

	if (!slug || slug.length !== 12 || !/^\d{12}$/.test(slug)) {
		throw error(400, `Invalid slug format. Expected 12 digits (YYYYMMDDHHmm) ${slug}`);
	}

	const year = slug.slice(0, 4);
	const month = slug.slice(4, 6);
	const day = slug.slice(6, 8);
	const hour = slug.slice(8, 10);
	const minute = slug.slice(10, 12);

	return {
		dataset,
		slug,
		direction: flowDirection.direction,
		fileInfo: {
			year,
			month,
			day,
			hour,
			minute,
			filename: `nfcapd.${slug}`
		}
	};
};
