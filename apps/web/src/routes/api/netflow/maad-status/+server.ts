import type { RequestHandler } from './$types';
import type { MaadStatusResponse } from '#lib/types/dimension-stats.ts';
import { getDatasetConfig, getRequestedDataset, withDatasetDb } from '#lib/server/datasets.ts';

export const GET: RequestHandler = async ({ url }) => {
	try {
		const dataset = await getRequestedDataset(url);
		const config = await getDatasetConfig(dataset);
		return await withDatasetDb(dataset, async ({ db }) => {
			const row = await db.get<{ computed: number }>(
				'SELECT EXISTS(SELECT 1 FROM maad_q_grid) AS computed'
			);
			const response: MaadStatusResponse = {
				computed: row?.computed === 1,
				internalSide: config.maadInternalSide === 1
			};
			return Response.json(response);
		});
	} catch (error) {
		console.error('Failed to read MAAD status:', error);
		return Response.json({ error: 'Database query failed' }, { status: 500 });
	}
};
