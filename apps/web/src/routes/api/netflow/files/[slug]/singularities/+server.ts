import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';

export const GET: RequestHandler = async () => {
	return json({ error: 'Singularities are not available from stored flow stats' }, { status: 410 });
};
