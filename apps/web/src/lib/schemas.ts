// schemas.ts
import { FLOW_DIRECTIONS } from '$lib/types/types';
import { z } from 'zod';
// Requires Zod 4+

export function createDateRangeSearchSchema(defaultStartDate: string) {
	return z.object({
		startDate: z.iso.date().default(defaultStartDate),
		endDate: z.iso.date().default(new Date().toJSON().slice(0, 10)),
		groupBy: z.enum(['date', 'hour', '30min', '10min', '5min']).default('date'),
		direction: z.enum(FLOW_DIRECTIONS).default('all')
	});
}
