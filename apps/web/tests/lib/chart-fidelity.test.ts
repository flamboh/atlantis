import { describe, expect, it } from 'vitest';
import { createChartScene } from '@tanstack/charts/scene';
import { renderChartSvg } from '@tanstack/charts/svg';
import {
	buildChartDefinition,
	type PlotOptions,
	type PlotSeries
} from '../../src/lib/components/charts/chart-registry';

function draw(series: PlotSeries[], options: Partial<PlotOptions> = {}) {
	const scene = createChartScene(
		buildChartDefinition(series, { xTitle: 'x', yTitle: 'y', ...options }, new Set(), () => {}),
		{ width: 800, height: 400 }
	);
	return { scene, svg: renderChartSvg(scene, { ariaLabel: 'Fidelity' }) };
}

describe('chart styling', () => {
	it('retains opaque 3px traffic/router lines and 60% stacked fills', () => {
		const data = [
			{ x: 0, y: 1 },
			{ x: 1, y: 2 }
		];
		const series = [{ label: 'router', color: '#2563eb', data }];
		const line = draw(series).svg;
		expect(line).toMatch(/stroke="#2563eb"[^>]*stroke-width="3"/);
		const stack = draw(series, { kind: 'stacked' }).svg;
		expect(stack).toContain('fill-opacity="0.6"');
		expect(stack).toMatch(/stroke="#2563eb"[^>]*stroke-width="3"/);
	});
	it('preserves filled spectrum points, a 2px line and a faint diagonal', () => {
		const { scene, svg } = draw(
			[
				{
					label: 'f(alpha)',
					color: '#9333ea',
					data: [
						{ x: 0.3, y: 0.2 },
						{ x: 0.6, y: 0.5 }
					],
					radius: 3,
					strokeWidth: 2,
					pointFill: '#9333ea',
					pointStroke: '#fff',
					pointStrokeWidth: 1
				}
			],
			{
				zero: false,
				annotations: [
					{
						data: [
							{ x: 0.3, y: 0.3 },
							{ x: 0.6, y: 0.6 }
						],
						color: '#808080',
						opacity: 0.5,
						strokeWidth: 1,
						dash: '5 5'
					}
				]
			}
		);
		expect(svg).toMatch(/<circle[^>]*fill="#9333ea"[^>]*stroke="#fff"[^>]*stroke-width="1"/);
		expect(svg).toMatch(/stroke="#9333ea"[^>]*stroke-width="2"/);
		expect(svg).toMatch(/stroke="#808080"[^>]*stroke-opacity="0.5"[^>]*stroke-width="1"/);
		expect(scene.points.map((point) => [point.datum.x, point.datum.y])).toEqual([
			[0.3, 0.2],
			[0.6, 0.5]
		]);
	});
});
