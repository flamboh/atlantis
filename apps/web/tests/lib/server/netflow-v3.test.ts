import { describe, expect, it } from 'vitest';
import {
	buildSpectrumPoints,
	buildStructurePoints,
	decodeF32,
	getNetflowSchemaVersion,
	groupByToGranularity,
	parseAggregateStatsParams,
	parseIpGranularity,
	parseIpGranularityOrDefault,
	parseFlowDirection,
	parseFlowDirectionParams,
	parseMaadIpVersion,
	parseMaadMeasure,
	parseMaadStatsParams,
	parseSourceIds,
	parseTimestamp,
	resolveSourceIds
} from '../../../src/lib/server/netflow-v3';

function f32(values: number[]): Uint8Array {
	return new Uint8Array(Float32Array.from(values).buffer);
}

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
		for (const param of ['abc', '0x6', '6.0', '6e0', ' 6', '']) {
			expect(
				parseMaadIpVersion(
					new URL(`http://localhost/api/test?ipVersion=${encodeURIComponent(param)}`)
				),
				param
			).toEqual({
				error: 'Invalid ipVersion. Expected one of: 4, 6',
				status: 400
			});
		}
	});

	it('parses the MAAD measure with an addresses default', () => {
		expect(parseMaadMeasure(new URL('http://localhost/api/test'))).toBe('addresses');
		for (const measure of ['addresses', 'packets', 'bytes']) {
			expect(parseMaadMeasure(new URL(`http://localhost/api/test?measure=${measure}`))).toBe(
				measure
			);
		}
		expect(parseMaadMeasure(new URL('http://localhost/api/test?measure=Packets'))).toEqual({
			error: 'Invalid measure. Expected one of: addresses, packets, bytes',
			status: 400
		});
	});

	it('extends aggregate stats params with the MAAD ip version and measure', () => {
		expect(
			parseMaadStatsParams(
				new URL(
					'http://localhost/api/test?routers=r1&startDate=100&endDate=200&direction=egress&ipVersion=6&measure=bytes'
				)
			)
		).toEqual({
			routers: ['r1'],
			granularity: '1h',
			start: 100,
			end: 200,
			direction: 'egress',
			srcLocality: 'internal',
			dstLocality: 'external',
			ipVersion: 6,
			measure: 'bytes'
		});
		expect(
			parseMaadStatsParams(
				new URL('http://localhost/api/test?routers=r1&startDate=100&endDate=200&measure=flows')
			)
		).toEqual({
			error: 'Invalid measure. Expected one of: addresses, packets, bytes',
			status: 400
		});
		expect(
			parseMaadStatsParams(new URL('http://localhost/api/test?startDate=100&endDate=200'))
		).toEqual({ error: 'No routers selected', status: 400 });
	});

	it('decodes little-endian f32 blobs from every accepted input shape', () => {
		expect(decodeF32(null)).toBeNull();
		expect(decodeF32(f32([]))).toEqual(new Float32Array([]));
		expect(decodeF32(f32([1, 2, 3]))).toEqual(new Float32Array([1, 2, 3]));
		expect(decodeF32(new Float32Array([1.5, -2.25]).buffer as ArrayBuffer)).toEqual(
			new Float32Array([1.5, -2.25])
		);
		expect(decodeF32([0, 0, 128, 63])).toEqual(new Float32Array([1]));

		const padded = Buffer.concat([Buffer.from([0, 0, 0]), Buffer.from(f32([1, 2]))]);
		const misaligned = padded.subarray(3);
		expect(misaligned.byteOffset % 4).not.toBe(0);
		expect(decodeF32(misaligned)).toEqual(new Float32Array([1, 2]));
	});

	it('builds structure points from tau/tau_sd blobs and a q grid, or an empty array for a null result', () => {
		const qGrid = { ipVersion: 4 as const, qMin: -0.5, qStep: 0.5, qCount: 3 };
		expect(buildStructurePoints(f32([0.2, 0.4]), f32([0.01, 0.02]), qGrid)).toEqual([
			{ q: -0.5, tau: Math.fround(0.2), sd: Math.fround(0.01) },
			{ q: 0, tau: Math.fround(0.4), sd: Math.fround(0.02) }
		]);
		expect(buildStructurePoints(f32([0.2]), null, qGrid)).toEqual([
			{ q: -0.5, tau: Math.fround(0.2), sd: 0 }
		]);
		expect(buildStructurePoints(null, null, qGrid)).toEqual([]);
		expect(buildStructurePoints(f32([0.25]), f32([0.5]), null)).toEqual([]);
	});

	it('builds spectrum points from interleaved alpha/f blobs, and distinguishes empty from not computed', () => {
		expect(buildSpectrumPoints(f32([1, 2, 3, 4]))).toEqual([
			{ alpha: 1, f: 2 },
			{ alpha: 3, f: 4 }
		]);
		expect(buildSpectrumPoints(f32([]))).toEqual([]);
		expect(buildSpectrumPoints(null)).toBeNull();
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
