// schemas.ts
import { FLOW_VISIBILITIES } from '$lib/types/types';
import { z } from 'zod';
// Requires Zod 4+

export function createDateRangeSearchSchema(defaultStartDate: string) {
	return z.object({
		startDate: z.iso.date().default(defaultStartDate),
		endDate: z.iso.date().default(new Date().toJSON().slice(0, 10)),
		groupBy: z.enum(['date', 'hour', '30min', '5min']).default('date'),
		srcVisibility: z.enum(FLOW_VISIBILITIES).default('all'),
		dstVisibility: z.enum(FLOW_VISIBILITIES).default('all')
	});
}
