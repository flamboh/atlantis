import { describe, expect, it } from 'vitest';
import { createDateRangeSearchSchema } from '../../src/lib/schemas';

describe('date range search params', () => {
	const schema = createDateRangeSearchSchema('2026-03-01');

	it('accepts every dashboard grouping including 10 minutes', () => {
		for (const groupBy of ['date', 'hour', '30min', '10min', '5min']) {
			expect(schema.parse({ groupBy }).groupBy).toBe(groupBy);
		}
	});

	it('defaults the MAAD IP family to IPv4 and accepts IPv6', () => {
		expect(schema.parse({}).ipVersion).toBe(4);
		expect(schema.parse({ ipVersion: 6 }).ipVersion).toBe(6);
	});

	it('rejects unknown MAAD IP families', () => {
		expect(schema.safeParse({ ipVersion: 5 }).success).toBe(false);
		expect(schema.safeParse({ ipVersion: '6' }).success).toBe(false);
	});

	it('rejects stored granularity spellings as groupings', () => {
		expect(schema.safeParse({ groupBy: '10m' }).success).toBe(false);
	});

	it('defaults the MAAD measure to addresses and accepts weighted measures', () => {
		expect(schema.parse({}).measure).toBe('addresses');
		for (const measure of ['addresses', 'packets', 'bytes']) {
			expect(schema.parse({ measure }).measure).toBe(measure);
		}
	});

	it('rejects unknown MAAD measures', () => {
		expect(schema.safeParse({ measure: 'flows' }).success).toBe(false);
	});
});
