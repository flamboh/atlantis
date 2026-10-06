import type { Chart, Plugin } from 'chart.js';
import { createChartContract, type RenderedChart } from './chart-contract';

function isHitBox(
	value: unknown
): value is { left: number; top: number; width: number; height: number } {
	return (
		typeof value === 'object' &&
		value !== null &&
		['left', 'top', 'width', 'height'].every(
			(key) => key in value && typeof Reflect.get(value, key) === 'number'
		)
	);
}

const contracts = new WeakMap<Chart, ReturnType<typeof createChartContract>>();

export const chartContractPlugin: Plugin = {
	id: 'chartContract',
	afterRender(chart) {
		const series = chart.data.datasets.map((dataset, index) => {
			const meta = chart.getDatasetMeta(index);
			let count = 0;
			let min = Infinity;
			let max = -Infinity;
			let total = 0;
			let target: { x: number; y: number } | undefined;
			for (let point = 0; point < dataset.data.length; point += 1) {
				const parsed = meta._parsed[point];
				if (
					!parsed ||
					typeof parsed !== 'object' ||
					!('x' in parsed) ||
					!('y' in parsed) ||
					typeof parsed.x !== 'number' ||
					typeof parsed.y !== 'number' ||
					!Number.isFinite(parsed.y)
				)
					continue;
				if (meta.xScale && (parsed.x < meta.xScale.min || parsed.x > meta.xScale.max)) continue;
				const element = meta.data[point];
				if (!element || !Number.isFinite(element.x) || !Number.isFinite(element.y)) continue;
				const values = 'chartContractValues' in dataset ? dataset.chartContractValues : null;
				const value = Array.isArray(values) ? values[point] : parsed.y;
				if (typeof value !== 'number' || !Number.isFinite(value)) continue;
				if (!target) target = { x: element.x / chart.width, y: element.y / chart.height };
				count += 1;
				min = Math.min(min, value);
				max = Math.max(max, value);
				total += value;
			}
			const legendIndex =
				chart.legend?.legendItems?.findIndex((item) => item.datasetIndex === index) ?? -1;
			const hitBoxes =
				chart.legend && 'legendHitBoxes' in chart.legend ? chart.legend.legendHitBoxes : null;
			const candidate =
				chart.options.plugins?.legend?.display && Array.isArray(hitBoxes)
					? hitBoxes[legendIndex]
					: null;
			const box = isHitBox(candidate) ? candidate : null;
			return {
				point: chart.isDatasetVisible(index) ? target : undefined,
				legend: box
					? {
							x: (box.left + box.width / 2) / chart.width,
							y: (box.top + box.height / 2) / chart.height
						}
					: undefined,
				label: dataset.label ?? 'Spectrum',
				visible: chart.isDatasetVisible(index),
				count,
				min: count ? min : null,
				max: count ? max : null,
				total
			};
		});
		const snapshot: RenderedChart = {
			name: chart.canvas.getAttribute('aria-label') ?? 'Chart',
			kind: chart.data.datasets.some((dataset) => 'fill' in dataset && dataset.fill)
				? 'stacked'
				: (chart.getDatasetMeta(0)?.type ?? 'line'),
			axes: Object.values(chart.scales).flatMap((scale) => {
				const title = 'title' in scale.options ? scale.options.title : null;
				return title &&
					typeof title === 'object' &&
					'display' in title &&
					title.display &&
					'text' in title
					? [String(title.text)]
					: [];
			}),
			series
		};
		let contract = contracts.get(chart);
		if (!contract) {
			contract = createChartContract(chart.canvas);
			contracts.set(chart, contract);
		}
		contract.render(snapshot);
	},
	afterDraw(chart) {
		const tooltip = chart.tooltip;
		const text = tooltip?.opacity
			? [
					...tooltip.title,
					...tooltip.beforeBody,
					...tooltip.body.flatMap((body) => [...body.before, ...body.lines, ...body.after]),
					...tooltip.afterBody,
					...tooltip.footer
				].join('\n')
			: '';
		contracts.get(chart)?.tooltip(text);
	},
	afterDestroy(chart) {
		contracts.get(chart)?.destroy();
		contracts.delete(chart);
	}
};
