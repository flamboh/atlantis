import { expect, it } from 'vitest';
import { createChartScene, defineChart } from '@tanstack/charts/scene';
import { scaleLinear } from '@tanstack/charts/scales/linear';
import { renderChartSvg } from '@tanstack/charts/svg';
import { temporalGrid } from '../../src/lib/components/charts/temporal-grid';

it('batches dense bucket gridlines into two paths with at most one stroke per pixel', () => {
	const ticks = Array.from({ length: 100000 }, (_, index) => index);
	const scene = createChartScene(
		defineChart({
			marks: [temporalGrid(ticks, new Set([0, 50000]))],
			scales: {
				x: { scale: scaleLinear().domain([0, 99999]), axis: false, grid: false },
				y: { scale: scaleLinear().domain([0, 1]), axis: false, grid: false }
			}
		}),
		{ width: 600, height: 300 }
	);
	const svg = renderChartSvg(scene, { ariaLabel: 'Grid' });
	expect(svg.match(/<path /g)).toHaveLength(2);
	expect(svg.match(/M[\d.]+,/g)!.length).toBeLessThanOrEqual(601);
	expect(svg).toContain('var(--chart-grid-highlight-color)');
	expect(svg).toContain('var(--chart-grid-color)');
	expect(scene.points).toHaveLength(0);
});
