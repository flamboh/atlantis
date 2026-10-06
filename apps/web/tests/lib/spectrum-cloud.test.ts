import { describe, expect, it } from 'vitest';
import { createChartScene, defineChart } from '@tanstack/charts/scene';
import { scaleLinear } from '@tanstack/charts/scales/linear';
import { scaleSequential } from 'd3-scale';
import { spectrumCloud } from '#lib/components/charts/spectrum-cloud.ts';

describe('dense spectrum scene', () => {
	it('keeps every finite observation, color, bucket identity and position without SVG point geometry', () => {
		const data = Array.from({ length: 3000 }, (_, index) => ({
			x: index % 3,
			y: index / 3000,
			f: index / 3000,
			label: `bucket-${index % 3}`
		}));
		const scene = createChartScene(
			defineChart({
				marks: [
					spectrumCloud([...data, { x: 0, y: NaN, f: 0, label: 'invalid' }], 'series-0', 'Spectrum')
				],
				scales: {
					x: { scale: scaleLinear().domain([0, 2]) },
					y: { scale: scaleLinear().domain([0, 1]) }
				},
				color: { scale: scaleSequential((value) => `rgb(${value * 255},0,0)`).domain([0, 1]) },
				focusRing: false
			}),
			{ width: 600, height: 300 }
		);
		expect(scene.points).toHaveLength(3000);
		expect(new Set(scene.points.map((point) => point.key)).size).toBe(3000);
		for (const index of [0, 1499, 2999]) {
			const point = scene.points[index];
			expect(point.datum).toBe(data[index]);
			expect(point.xValue).toBe(index % 3);
			expect(point.yValue).toBe(index / 3000);
			expect(point.color).toBe(`rgb(${(index / 3000) * 255},0,0)`);
			expect(point.x).toBeCloseTo(scene.chart.x + ((index % 3) / 2) * scene.chart.width);
			expect(point.y).toBeCloseTo(scene.chart.y + (1 - index / 3000) * scene.chart.height);
		}
	});
});
