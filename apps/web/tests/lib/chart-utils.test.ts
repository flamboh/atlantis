import { describe, expect, it } from 'vitest';
import {
	clampGroupByToDateRange,
	getMaxAllowedGranularityForDateRange,
	isGranularityAllowedForDateRange
} from '../../src/lib/components/charts/chart-utils';
import {
	formatIpGranularityTick,
	formatTemporalBucketLabel,
	shouldHighlightIpGranularityGrid
} from '../../src/lib/components/charts/ip-time-axis';
import { dateStringToEpochPST } from '../../src/lib/utils/timezone';

describe('chart granularity policy', () => {
	it('allows 5 minute granularity for short ranges', () => {
		expect(getMaxAllowedGranularityForDateRange('2026-03-01', '2026-03-03')).toBe('5min');
		expect(isGranularityAllowedForDateRange('5min', '2026-03-01', '2026-03-03')).toBe(true);
	});

	it('disables 5 minute granularity once the range exceeds the adaptive cutoff', () => {
		expect(getMaxAllowedGranularityForDateRange('2026-03-01', '2026-03-05')).toBe('30min');
		expect(isGranularityAllowedForDateRange('5min', '2026-03-01', '2026-03-05')).toBe(false);
		expect(isGranularityAllowedForDateRange('30min', '2026-03-01', '2026-03-05')).toBe(true);
	});

	it('clamps an invalid selection to the finest allowed granularity', () => {
		expect(clampGroupByToDateRange('5min', '2026-03-01', '2026-03-25')).toBe('hour');
		expect(clampGroupByToDateRange('30min', '2026-03-01', '2026-06-15')).toBe('date');
	});
});

describe('shared IP granularity chart labels', () => {
	it('formats bucket labels using Pacific time', () => {
		const bucketStart = dateStringToEpochPST('2026-03-02');

		expect(formatTemporalBucketLabel(bucketStart, '1d')).toBe('2026-03-02');
		expect(formatTemporalBucketLabel(bucketStart, '1h')).toBe('2026-03-02 00:00');
	});

	it('matches existing tick and grid highlight policy', () => {
		const mondayStart = dateStringToEpochPST('2026-03-02');
		const tuesdayStart = dateStringToEpochPST('2026-03-03');

		expect(formatIpGranularityTick(mondayStart, '1d', 0)).toBe('Mon 03/02');
		expect(formatIpGranularityTick(tuesdayStart, '1d', 1)).toBe('');
		expect(shouldHighlightIpGranularityGrid(mondayStart, '1d', 0)).toBe(true);
		expect(shouldHighlightIpGranularityGrid(tuesdayStart, '1d', 1)).toBe(false);
	});
});
