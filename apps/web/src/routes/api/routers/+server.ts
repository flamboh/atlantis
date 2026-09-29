import type { RequestHandler } from './$types';
import { getRequestedDataset, listDatasetSources } from '#lib/server/datasets.ts';

export const GET: RequestHandler = async ({ url }) => {
	try {
		const dataset = await getRequestedDataset(url);
		const routers = await listDatasetSources(dataset);
		if (routers.length === 0) {
			return Response.json(
				{ error: `No routers available for dataset '${dataset}'` },
				{ status: 404 }
			);
		}
		return Response.json(routers);
	} catch (error) {
		console.error('Failed to list routers:', error);
		return Response.json({ error: 'No routers available' }, { status: 500 });
	}
};
