import { describe, expect, it } from 'vitest';
import {
	getBucketStartQuery,
	getNetflowSchemaVersion,
	groupByToGranularity,
	normalizeStructurePoints,
	parseAggregateStatsParams,
	parseIpGranularity,
	parseIpGranularityOrDefault,
	parseFlowVisibility,
	parseSourceIds,
	parseTimestamp,
	resolveSourceIds
} from '../../../src/lib/server/netflow-v3';

describe('netflow v3 helpers', () => {
	it('is v3-only', () => {
		expect(getNetflowSchemaVersion()).toBe('v3');
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

	it('parses flow visibility request values without coercing invalid values', () => {
		expect(parseFlowVisibility('literal')).toBe('literal');
		expect(parseFlowVisibility('anonymized')).toBe('anonymized');
		expect(parseFlowVisibility(null)).toBeNull();
		expect(parseFlowVisibility('bad')).toBeNull();
	});

	it('validates aggregate stats request params', () => {
		expect(
			parseAggregateStatsParams(
				new URL('http://localhost/api/test?routers=r1,r2&granularity=30m&startDate=100&endDate=200')
			)
		).toEqual({
			routers: ['r1', 'r2'],
			granularity: '30m',
			start: 100,
			end: 200,
			srcVisibility: 'all',
			dstVisibility: 'all'
		});

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
		expect(
			parseAggregateStatsParams(
				new URL('http://localhost/api/test?routers=r1&startDate=100&endDate=200&srcVisibility=bad')
			)
		).toEqual({
			error: 'Invalid srcVisibility. Expected one of: all, literal, anonymized',
			status: 400
		});
	});

	it('keeps raw bucket starts for 5 minute requests', () => {
		expect(getBucketStartQuery('bucket_start', '5min')).toBe('bucket_start');
	});

	it('normalizes structure points from MAAD variants', () => {
		expect(normalizeStructurePoints([{ q: 1, tauTilde: 2, s: 3 }])).toEqual([
			{ q: 1, tau: 2, sd: 3 }
		]);
	});

	it('resolves additive sources to one disjoint physical cover', () => {
		const definitions = [
			{ sourceId: 'cc_ir1_gw', members: ['cc_ir1_gw'] },
			{ sourceId: 'oh_ir1_gw', members: ['oh_ir1_gw'] },
			{ sourceId: 'uoregon_all', members: ['cc_ir1_gw', 'oh_ir1_gw'] }
		];

		expect(resolveSourceIds(definitions, ['cc_ir1_gw', 'oh_ir1_gw'])).toEqual(['uoregon_all']);
		expect(resolveSourceIds(definitions, ['cc_ir1_gw', 'uoregon_all'])).toEqual(['uoregon_all']);
		expect(resolveSourceIds(definitions, ['cc_ir1_gw'])).toEqual(['cc_ir1_gw']);
	});
});
