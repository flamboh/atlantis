import type { RequestHandler } from './$types';
import { listDatasetSummaries } from '#lib/server/datasets.ts';
import type { DatasetSummariesResponse } from '#lib/types/types.ts';

export const GET: RequestHandler = async () => {
	try {
		const response: DatasetSummariesResponse = {
			data: await listDatasetSummaries(),
			error: null
		};
		return Response.json(response);
	} catch (error) {
		console.error('Failed to list datasets:', error);
		const response: DatasetSummariesResponse = {
			data: null,
			error: 'Failed to list datasets'
		};
		return Response.json(response, { status: 500 });
	}
};
