import { expect, it } from 'vitest';
import { rect } from '@tanstack/charts/rect';
import { createChartScene, defineChart, findNearestPoint } from '@tanstack/charts/scene';
import { scaleLinear } from '@tanstack/charts/scales/linear';
import { renderCoverageSvg } from '#lib/components/charts/coverage-marks.ts';
import type { PlotPoint } from '#lib/components/charts/chart-registry.ts';

it('retains each coverage interval and ratio when batching the painted segments', () => {
	const data: PlotPoint[] = [
		{
			x: 0,
			x2: 10,
			y: 0,
			value: 1,
			coverage: { state: 'complete', observedUnits: 10, expectedUnits: 10 }
		},
		{
			x: 10,
			x2: 20,
			y: 0,
			value: 1,
			coverage: { state: 'complete', observedUnits: 10, expectedUnits: 10 }
		}
	];
	const scene = createChartScene(
		defineChart({
			marks: [
				rect(data, {
					id: 'coverage',
					x: 'x',
					x1: 'x',
					x2: 'x2',
					y: 'y',
					y1: () => -0.1,
					y2: () => 0.1,
					fill: 'green',
					inset: 0
				})
			],
			scales: {
				x: { scale: scaleLinear().domain([0, 20]) },
				y: { scale: scaleLinear().domain([-1, 1]) }
			},
			focusRing: false
		}),
		{ width: 400, height: 100 }
	);
	const svg = renderCoverageSvg(scene, { ariaLabel: 'Coverage' });
	expect(svg).toContain('coverage-segments');
	expect(svg).not.toContain('<rect');
	expect(scene.points).toHaveLength(2);
	for (const point of scene.points) {
		for (const fraction of [0.01, 0.5, 0.99]) {
			const x =
				scene.scales.x.map(Number(point.x1Value)) +
				(scene.scales.x.map(Number(point.x2Value)) - scene.scales.x.map(Number(point.x1Value))) *
					fraction;
			expect(findNearestPoint(scene, x, point.y, 48)).toBe(point);
		}
	}
	expect(scene.points.map((point) => [point.x1Value, point.x2Value, point.datum.value])).toEqual([
		[0, 10, 1],
		[10, 20, 1]
	]);
	expect(scene.points[0].datum).toBe(data[0]);
	expect(scene.points[1].datum).toBe(data[1]);
	expect(scene.points[0].x).toBeCloseTo(scene.chart.x + scene.chart.width / 4);
	expect(scene.points[1].x).toBeCloseTo(scene.chart.x + (scene.chart.width * 3) / 4);
});
