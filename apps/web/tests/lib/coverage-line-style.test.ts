import { describe, expect, it } from 'vitest';
import { buildCoveragePointStyle } from '../../src/lib/components/charts/coverage-line-style';

describe('coverage-aware line point styles', () => {
	const completeCoverage = { state: 'complete' as const };
	const partialCoverage = { state: 'partial' as const };
	const unknownCoverage = { state: 'unknown' as const };

	it('draws an isolated numeric partial bucket as a hollow point', () => {
		const points = [
			{ value: null, coverage: unknownCoverage },
			{ value: 4, coverage: partialCoverage },
			{ value: null, coverage: unknownCoverage },
			{ value: 0, coverage: completeCoverage },
			{ value: null, coverage: partialCoverage }
		];

		expect(
			buildCoveragePointStyle(
				points,
				(point) => point.value,
				(point) => point.coverage,
				'rgb(54, 162, 235)'
			)
		).toEqual({
			pointRadius: [0, 3, 0, 0, 0],
			pointBackgroundColor: [
				'rgb(54, 162, 235)',
				'rgba(0, 0, 0, 0)',
				'rgb(54, 162, 235)',
				'rgb(54, 162, 235)',
				'rgb(54, 162, 235)'
			],
			pointBorderColor: 'rgb(54, 162, 235)',
			pointBorderWidth: [0, 2, 0, 0, 0]
		});
	});

	it('returns no point styles when no numeric partial bucket exists', () => {
		expect(
			buildCoveragePointStyle(
				[
					{ value: 0, coverage: completeCoverage },
					{ value: null, coverage: unknownCoverage }
				],
				(point) => point.value,
				(point) => point.coverage,
				'blue'
			)
		).toBeNull();
	});
});
