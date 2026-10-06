import { renderChartSvg } from '@tanstack/charts/svg';
import type { SceneNode } from '@tanstack/charts';

export const renderCoverageSvg: typeof renderChartSvg = (scene, options) => {
	function batch(nodes: readonly SceneNode[]): SceneNode[] {
		return nodes.map((node) => {
			if (node.kind !== 'group') return node;
			if (node.className !== 'ts-chart__rect') return { ...node, children: batch(node.children) };
			const rectangles = node.children.filter((child) => child.kind === 'rect');
			return {
				...node,
				children: rectangles.length
					? [
							{
								kind: 'area',
								key: `${node.key}-segments`,
								points: [],
								style: rectangles[0].style,
								path: rectangles
									.map(({ x, y, width, height }) => {
										const round = (value: number) => Math.round(value * 100) / 100;
										return `M${round(x)},${round(y)}h${round(width)}v${round(height)}h${-round(width)}Z`;
									})
									.join('')
							}
						]
					: []
			};
		});
	}
	return renderChartSvg({ ...scene, nodes: batch(scene.nodes) }, options);
};
