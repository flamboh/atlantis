import { describe, expect, it } from 'vitest';
import { coverageLineRuns } from '../../src/lib/components/charts/coverage-line-style';

describe('coverage-aware line segments', () => {
	const runs = (points: Array<{ y: number | null; partial: boolean }>) =>
		coverageLineRuns(
			points,
			(point) => point.y,
			(point) => point.partial
		);
	it('dashes both segments adjoining a partial observation without bridging unknown gaps', () => {
		const points = [
			{ y: 0, partial: false },
			{ y: 4, partial: true },
			{ y: 8, partial: false },
			{ y: 9, partial: false },
			{ y: null, partial: false },
			{ y: 10, partial: false }
		];
		expect(runs(points)).toEqual([
			{ points: points.slice(0, 3), partial: true },
			{ points: points.slice(2, 4), partial: false },
			{ points: points.slice(5), partial: false }
		]);
	});
	it('retains singleton zero observations and breaks nonfinite gaps', () => {
		const points = [
			{ y: null, partial: false },
			{ y: 0, partial: true },
			{ y: NaN, partial: false }
		];
		expect(runs(points)).toEqual([{ points: [points[1]], partial: true }]);
	});
});
