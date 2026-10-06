import { describe, expect, it } from 'vitest';
import { temporalTicks } from '../../src/lib/components/charts/temporal-ticks';
import { formatIpGranularityTick } from '../../src/lib/components/charts/ip-time-axis';
import { formatNetflowTick, formatLabels } from '../../src/lib/components/charts/chart-utils';
import { dateStringToEpochPST } from '../../src/lib/utils/timezone';
import type { GroupByOption, NetflowDataPoint } from '../../src/lib/components/netflow/types';

describe('temporal ticks', () => {
	const monday = dateStringToEpochPST('2025-06-02');
	it.each([
		['1d', 86400, 14, [0, 7]],
		['1h', 3600, 48, [0, 24]],
		['30m', 1800, 96, [0, 24, 48, 72]],
		['10m', 600, 288, [0, 72, 144, 216]],
		['5m', 300, 24, [0, 12]]
	] as const)(
		'preserves %s calendar boundaries and formatting',
		(granularity, step, count, indexes) => {
			const starts = Array.from({ length: count }, (_, index) => monday + index * step);
			const actual = temporalTicks(starts, (value) =>
				formatIpGranularityTick(value, granularity, 0)
			);
			expect(actual).toEqual(indexes.map((index) => monday + index * step));
		}
	);
	it('keeps every midnight/noon tick on a seven-day range', () => {
		const starts = Array.from({ length: 1008 }, (_, index) => monday + index * 600);
		const ticks = temporalTicks(starts, (value) => formatIpGranularityTick(value, '10m', 0));
		expect(ticks).toHaveLength(14);
		expect(ticks.map((value) => formatIpGranularityTick(value, '10m', 0)).slice(0, 3)).toEqual([
			'Mon 6/2 00:00',
			'Mon 6/2 12:00',
			'Tue 6/3 00:00'
		]);
		const compact = temporalTicks(
			starts,
			(value) => formatIpGranularityTick(value, '10m', 0),
			true
		);
		expect(compact).toHaveLength(4);
		expect(compact[0]).toBe(monday);
		expect(compact[3]).toBe(monday + 13 * 43200);
	});
	it('does not invent a label when no boundary exists in the visible range', () => {
		expect(
			temporalTicks([monday + 300], (value) => formatIpGranularityTick(value, '5m', 0))
		).toEqual([]);
	});
	it.each(['date', 'hour', '30min', '10min', '5min'] as GroupByOption[])(
		'keeps traffic and IP labels on the same %s boundaries',
		(groupBy) => {
			const starts = Array.from({ length: 2016 }, (_, index) => monday + index * 300);
			const data: NetflowDataPoint[] = starts.map((bucketStart) => ({
				bucketStart,
				bucketEnd: bucketStart + 300,
				coverage: { state: 'complete', observedUnits: 1, expectedUnits: 1 },
				data: null
			}));
			const labels = formatLabels(data, groupBy);
			const map = new Map(starts.map((start, index) => [start, labels[index]]));
			const granularity = {
				date: '1d',
				hour: '1h',
				'30min': '30m',
				'10min': '10m',
				'5min': '5m'
			} as const;
			expect(
				temporalTicks(starts, (value) => formatNetflowTick(groupBy, map.get(value), 0))
			).toEqual(
				temporalTicks(starts, (value) => formatIpGranularityTick(value, granularity[groupBy], 0))
			);
		}
	);
});
