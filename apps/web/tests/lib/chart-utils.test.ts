import { describe, expect, it } from 'vitest';
import {
	clampGroupByToDateRange,
	buildTemporalChartPoints,
	getCoveragePointStyle,
	getCoverageTooltipLines,
	isCoverageSegmentDashed,
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

describe('coverage-aware chart data', () => {
	const completeCoverage = { state: 'complete' as const, observedUnits: 12, expectedUnits: 12 };
	const partialCoverage = { state: 'partial' as const, observedUnits: 8, expectedUnits: 12 };
	const unknownCoverage = { state: 'unknown' as const, observedUnits: 0, expectedUnits: 12 };

	it('keeps bucket timestamps and preserves zero, partial, and unknown values', () => {
		const points = buildTemporalChartPoints(
			[
				{ bucketStart: 100, bucketEnd: 160, coverage: completeCoverage, data: { value: 0 } },
				{ bucketStart: 160, bucketEnd: 220, coverage: partialCoverage, data: { value: 4 } },
				{ bucketStart: 520, bucketEnd: 580, coverage: unknownCoverage, data: null }
			],
			(data) => data.value
		);

		expect(points.map(({ x, y }) => ({ x, y }))).toEqual([
			{ x: 100, y: 0 },
			{ x: 160, y: 4 },
			{ x: 520, y: null }
		]);
		expect(points[1]?.coverage).toEqual(partialCoverage);
	});

	it('styles partial points as hollow and dashes adjoining segments', () => {
		const partialPoint = { coverage: partialCoverage };
		const completePoint = { coverage: completeCoverage };

		expect(getCoveragePointStyle(partialCoverage, '#2563eb')).toEqual({
			backgroundColor: 'transparent',
			borderColor: '#2563eb',
			borderWidth: 2,
			radius: 4
		});
		expect(isCoverageSegmentDashed(partialPoint, completePoint)).toBe(true);
		expect(isCoverageSegmentDashed(completePoint, completePoint)).toBe(false);
	});

	it('exposes observed and expected units for partial coverage tooltips', () => {
		expect(getCoverageTooltipLines(partialCoverage)).toEqual([
			'Coverage: partial',
			'Observed units: 8 / 12'
		]);
		expect(getCoverageTooltipLines(completeCoverage)).toEqual([]);
	});
});
