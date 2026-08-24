import { describe, expect, it } from 'vitest';
import {
	formatCoverageState,
	formatCoverageStripLabel
} from '../../src/lib/components/charts/coverage-strip';
import {
	flattenCoverageTimelines,
	rebuildCoverageTimelines
} from '../../src/lib/components/charts/CoverageStrip.svelte';
import { dateStringToEpochPST } from '../../src/lib/utils/timezone';

describe('coverage strip labels', () => {
	it('uses the same Pacific-time bucket labels as the traffic chart', () => {
		const bucketStart = dateStringToEpochPST('2026-03-02');

		expect(formatCoverageStripLabel(bucketStart, 'date')).toBe('2026-03-02');
		expect(formatCoverageStripLabel(bucketStart, 'hour')).toBe('2026-03-02 00:00');
		expect(formatCoverageStripLabel(bucketStart + 5 * 60, '5min')).toBe('2026-03-02 00:05');
	});

	it('keeps coverage counts in partial details while keeping unknown concise', () => {
		expect(formatCoverageState({ state: 'complete', observedUnits: 12, expectedUnits: 12 })).toBe(
			'Complete coverage'
		);
		expect(formatCoverageState({ state: 'partial', observedUnits: 8, expectedUnits: 12 })).toBe(
			'Partial coverage · 8 / 12 units'
		);
		expect(formatCoverageState({ state: 'unknown', observedUnits: 0, expectedUnits: 12 })).toBe(
			'Unknown coverage'
		);
	});
});

describe('coverage strip window records', () => {
	it('round-trips source lanes and preserves unknown buckets when reading a cached window', () => {
		const timelines = [
			{
				sourceId: 'router-b',
				buckets: [
					{
						bucketStart: 200,
						bucketEnd: 300,
						coverage: { state: 'unknown' as const, observedUnits: 0, expectedUnits: 1 }
					}
				]
			},
			{
				sourceId: 'router-a',
				buckets: [
					{
						bucketStart: 100,
						bucketEnd: 200,
						coverage: { state: 'complete' as const, observedUnits: 1, expectedUnits: 1 }
					}
				]
			}
		];

		const rebuilt = rebuildCoverageTimelines(flattenCoverageTimelines(timelines), [
			'router-a',
			'router-b'
		]);

		expect(rebuilt.map((timeline) => timeline.sourceId)).toEqual(['router-a', 'router-b']);
		expect(rebuilt[0]?.buckets.map((bucket) => bucket.bucketStart)).toEqual([100]);
		expect(rebuilt[1]?.buckets[0]?.coverage.state).toBe('unknown');
	});
});
