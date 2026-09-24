import type { RequestHandler } from './$types';
import type { MaadStatusResponse } from '$lib/types/dimension-stats';
import { getRequestedDataset, withDatasetDb } from '$lib/server/datasets';

export const GET: RequestHandler = async ({ url, platform }) => {
	try {
		const dataset = await getRequestedDataset(url, platform);
		return await withDatasetDb(dataset, platform, async ({ db }) => {
			const row = await db.get<{ computed: number }>(
				'SELECT EXISTS(SELECT 1 FROM maad_q_grid) AS computed'
			);
			const response: MaadStatusResponse = { computed: row?.computed === 1 };
			return Response.json(response);
		});
	} catch (error) {
		console.error('Failed to read MAAD status:', error);
		return Response.json({ error: 'Database query failed' }, { status: 500 });
	}
};
