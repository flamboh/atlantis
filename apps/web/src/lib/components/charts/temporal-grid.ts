import type { ChartMark, SceneNode } from '@tanstack/charts';

export function temporalGrid(
	values: readonly number[],
	boundaries: ReadonlySet<number>
): ChartMark<never, number, number> {
	return {
		initialize: () => ({
			id: 'temporal-grid',
			channels: {},
			render: ({ scales, chart }) => {
				const columns = new Map<number, boolean>();
				for (const value of values) {
					const position = scales.x.map(value);
					if (position < chart.x || position > chart.x + chart.width || !Number.isFinite(position))
						continue;
					const x = Math.round(position) + 0.5;
					columns.set(x, (columns.get(x) ?? false) || boundaries.has(value));
				}
				const nodes: SceneNode[] = [false, true].map((major) => ({
					kind: 'polyline',
					key: `temporal-grid-${major ? 'major' : 'minor'}`,
					points: [],
					path: [...columns]
						.filter(([, boundary]) => boundary === major)
						.map(([x]) => `M${x},${chart.y}V${chart.y + chart.height}`)
						.join(''),
					ariaHidden: true,
					style: {
						fill: 'none',
						stroke: major ? 'var(--chart-grid-color)' : 'var(--chart-grid-highlight-color)',
						strokeWidth: 1
					}
				}));
				return { nodes };
			}
		})
	};
}
