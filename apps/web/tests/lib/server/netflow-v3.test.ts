import { describe, expect, it } from 'vitest';
import {
	getNetflowSchemaVersion,
	groupByToGranularity,
	normalizeStructurePoints,
	parseAggregateStatsParams,
	parseIpGranularity,
	parseIpGranularityOrDefault,
	parseFlowDirection,
	parseFlowDirectionParams,
	parseMaadIpVersion,
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
		expect(groupByToGranularity('10min')).toBe('10m');
		expect(groupByToGranularity('5min')).toBe('5m');
	});

	it('parses stored IP granularity request values', () => {
		expect(parseIpGranularity('5m')).toBe('5m');
		expect(parseIpGranularity('10m')).toBe('10m');
		expect(parseIpGranularity('10min')).toBeNull();
		expect(parseIpGranularity('bad')).toBeNull();
		expect(parseIpGranularityOrDefault(null)).toBe('1h');
		expect(parseIpGranularityOrDefault('bad')).toBe('1h');
	});

	it('parses flow direction request values without coercing invalid values', () => {
		expect(parseFlowDirection('ingress')).toBe('ingress');
		expect(parseFlowDirection('egress')).toBe('egress');
		expect(parseFlowDirection('lateral')).toBe('lateral');
		expect(parseFlowDirection('transit')).toBe('transit');
		expect(parseFlowDirection(null)).toBeNull();
		expect(parseFlowDirection('bad')).toBeNull();
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
			direction: 'all',
			srcLocality: 'all',
			dstLocality: 'all'
		});

		expect(
			parseAggregateStatsParams(
				new URL(
					'http://localhost/api/test?routers=r1,r2&granularity=30m&startDate=100&endDate=200&direction=ingress'
				)
			)
		).toEqual({
			routers: ['r1', 'r2'],
			granularity: '30m',
			start: 100,
			end: 200,
			direction: 'ingress',
			srcLocality: 'external',
			dstLocality: 'internal'
		});

		expect(
			parseAggregateStatsParams(
				new URL('http://localhost/api/test?routers=r1&granularity=10m&startDate=100&endDate=200')
			)
		).toMatchObject({ granularity: '10m' });

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
				new URL('http://localhost/api/test?routers=r1&granularity=weekly&startDate=100&endDate=200')
			)
		).toEqual({
			error: 'Invalid granularity. Expected one of: 5m, 10m, 30m, 1h, 1d',
			status: 400
		});
		expect(
			parseAggregateStatsParams(
				new URL('http://localhost/api/test?routers=r1&startDate=100&endDate=200&direction=bad')
			)
		).toEqual({
			error: 'Invalid direction. Expected one of: all, ingress, egress, lateral, transit',
			status: 400
		});
	});

	it('defaults direction to all and maps each direction to its locality pair', () => {
		expect(parseFlowDirectionParams(new URL('http://localhost/api/test'))).toEqual({
			direction: 'all',
			srcLocality: 'all',
			dstLocality: 'all'
		});
		expect(parseFlowDirectionParams(new URL('http://localhost/api/test?direction=all'))).toEqual({
			direction: 'all',
			srcLocality: 'all',
			dstLocality: 'all'
		});
		expect(
			parseFlowDirectionParams(new URL('http://localhost/api/test?direction=ingress'))
		).toEqual({ direction: 'ingress', srcLocality: 'external', dstLocality: 'internal' });
		expect(parseFlowDirectionParams(new URL('http://localhost/api/test?direction=egress'))).toEqual(
			{ direction: 'egress', srcLocality: 'internal', dstLocality: 'external' }
		);
		expect(
			parseFlowDirectionParams(new URL('http://localhost/api/test?direction=lateral'))
		).toEqual({ direction: 'lateral', srcLocality: 'internal', dstLocality: 'internal' });
		expect(
			parseFlowDirectionParams(new URL('http://localhost/api/test?direction=transit'))
		).toEqual({ direction: 'transit', srcLocality: 'external', dstLocality: 'external' });
		expect(parseFlowDirectionParams(new URL('http://localhost/api/test?direction=bogus'))).toEqual({
			error: 'Invalid direction. Expected one of: all, ingress, egress, lateral, transit',
			status: 400
		});
	});

	it('parses the MAAD ip version request param, defaulting to 4', () => {
		expect(parseMaadIpVersion(new URL('http://localhost/api/test'))).toBe(4);
		expect(parseMaadIpVersion(new URL('http://localhost/api/test?ipVersion=4'))).toBe(4);
		expect(parseMaadIpVersion(new URL('http://localhost/api/test?ipVersion=6'))).toBe(6);
		expect(parseMaadIpVersion(new URL('http://localhost/api/test?ipVersion=5'))).toEqual({
			error: 'Invalid ipVersion. Expected one of: 4, 6',
			status: 400
		});
		expect(parseMaadIpVersion(new URL('http://localhost/api/test?ipVersion=abc'))).toEqual({
			error: 'Invalid ipVersion. Expected one of: 4, 6',
			status: 400
		});
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
