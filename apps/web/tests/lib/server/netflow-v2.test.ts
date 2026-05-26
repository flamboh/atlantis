import { describe, expect, it } from 'vitest';
import {
	getBucketStartQuery,
	getNetflowSchemaVersion,
	groupByToGranularity,
	normalizeStructurePoints,
	parseAggregateStatsParams,
	parseIpGranularity,
	parseIpGranularityOrDefault,
	parseSourceIds,
	parseTimestamp
} from '../../../src/lib/server/netflow-v2';

describe('netflow v2 helpers', () => {
	it('is v2-only', () => {
		expect(getNetflowSchemaVersion()).toBe('v2');
	});

	it('parses request primitives', () => {
		expect(parseSourceIds(' r1, r2 ,, ')).toEqual(['r1', 'r2']);
		expect(parseTimestamp('123')).toBe(123);
		expect(parseTimestamp('not-a-number')).toBeNull();
	});

	it('maps groupings to stored granularities', () => {
		expect(groupByToGranularity('date')).toBe('1d');
		expect(groupByToGranularity('hour')).toBe('1h');
		expect(groupByToGranularity('30min')).toBe('30m');
		expect(groupByToGranularity('5min')).toBe('5m');
	});

	it('parses stored IP granularity request values', () => {
		expect(parseIpGranularity('5m')).toBe('5m');
		expect(parseIpGranularity('bad')).toBeNull();
		expect(parseIpGranularityOrDefault(null)).toBe('1h');
		expect(parseIpGranularityOrDefault('bad')).toBe('1h');
	});

	it('validates aggregate stats request params', () => {
		expect(
			parseAggregateStatsParams(
				new URL('http://localhost/api/test?routers=r1,r2&granularity=30m&startDate=100&endDate=200')
			)
		).toEqual({ routers: ['r1', 'r2'], granularity: '30m', start: 100, end: 200 });

		expect(
			parseAggregateStatsParams(new URL('http://localhost/api/test?startDate=100&endDate=200'))
		).toEqual({ error: 'No routers selected', status: 400 });
		expect(
			parseAggregateStatsParams(
				new URL('http://localhost/api/test?routers=r1&startDate=bad&endDate=200')
			)
		).toEqual({ error: 'Invalid start or end time', status: 400 });
		expect(
			parseAggregateStatsParams(
				new URL('http://localhost/api/test?routers=r1&startDate=200&endDate=100')
			)
		).toEqual({ error: 'Start time must be before end time', status: 400 });
	});

	it('keeps raw bucket starts for 5 minute requests', () => {
		expect(getBucketStartQuery('bucket_start', '5min')).toBe('bucket_start');
	});

	it('normalizes structure points from MAAD variants', () => {
		expect(normalizeStructurePoints([{ q: 1, tauTilde: 2, s: 3 }])).toEqual([
			{ q: 1, tau: 2, sd: 3 }
		]);
	});
});
