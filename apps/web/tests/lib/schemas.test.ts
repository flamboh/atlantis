import { describe, expect, it } from 'vitest';
import { createDateRangeSearchSchema } from '../../src/lib/schemas';

describe('date range search params', () => {
	const schema = createDateRangeSearchSchema('2026-03-01');

	it('accepts every dashboard grouping including 10 minutes', () => {
		for (const groupBy of ['date', 'hour', '30min', '10min', '5min']) {
			expect(schema.parse({ groupBy }).groupBy).toBe(groupBy);
		}
	});

	it('rejects stored granularity spellings as groupings', () => {
		expect(schema.safeParse({ groupBy: '10m' }).success).toBe(false);
	});
});
