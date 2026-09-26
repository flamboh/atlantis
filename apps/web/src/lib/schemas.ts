// schemas.ts
import {
	DEFAULT_MAAD_IP_VERSION,
	DEFAULT_MAAD_MEASURE,
	FLOW_DIRECTIONS,
	MAAD_IP_VERSIONS,
	MAAD_MEASURES
} from '$lib/types/types';
import { z } from 'zod';
// Requires Zod 4+

export function createDateRangeSearchSchema(defaultStartDate: string) {
	return z.object({
		startDate: z.iso.date().default(defaultStartDate),
		endDate: z.iso.date().default(new Date().toJSON().slice(0, 10)),
		groupBy: z.enum(['date', 'hour', '30min', '10min', '5min']).default('date'),
		direction: z.enum(FLOW_DIRECTIONS).default('all'),
		ipVersion: z.literal(MAAD_IP_VERSIONS).default(DEFAULT_MAAD_IP_VERSION),
		measure: z.enum(MAAD_MEASURES).default(DEFAULT_MAAD_MEASURE)
	});
}
