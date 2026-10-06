import type { ChartMark, ChartPoint, ChartRenderContext } from '@tanstack/charts';
import type { PlotPoint, PlotSeries } from './chart-registry';

export function spectrumCloud(
	data: PlotPoint[],
	id: string,
	label: string
): ChartMark<PlotPoint, number, number> {
	return {
		initialize: () => ({
			id,
			channels: {
				x: { scale: 'x', values: data.map((point) => point.x) },
				y: { scale: 'y', values: data.flatMap((point) => (point.y === null ? [] : [point.y])) },
				color: { scale: 'color', values: data.map((point) => point.f ?? 0) }
			},
			render: ({ scales, color }) => {
				const points: ChartPoint<PlotPoint, number, number>[] = [];
				for (const [datumIndex, datum] of data.entries()) {
					if (datum.y === null) continue;
					const x = scales.x.map(datum.x);
					const y = scales.y.map(datum.y);
					if (!Number.isFinite(x) || !Number.isFinite(y)) continue;
					points.push({
						key: `${id}:${datumIndex}`,
						markId: id,
						group: label,
						groupLabel: label,
						datum,
						datumIndex,
						xValue: datum.x,
						yValue: datum.y,
						x,
						y,
						color: color(datum.f ?? 0)
					});
				}
				return {
					points,
					nodes: [
						{
							kind: 'group',
							key: id,
							className: 'atlantis-spectrum-cloud',
							ariaHidden: true,
							children: []
						}
					]
				};
			}
		})
	};
}

export function paintSpectrumCloud(
	context: ChartRenderContext<PlotPoint, number, number>,
	series: PlotSeries[]
) {
	for (const group of context.svg.querySelectorAll<SVGGElement>('.atlantis-spectrum-cloud')) {
		const { width, height } = context.scene;
		const ratio = context.container.ownerDocument.defaultView?.devicePixelRatio ?? 1;
		const layer = document.createElementNS('http://www.w3.org/2000/svg', 'foreignObject');
		layer.setAttribute('x', '0');
		layer.setAttribute('y', '0');
		layer.setAttribute('width', String(width));
		layer.setAttribute('height', String(height));
		layer.setAttribute('pointer-events', 'none');
		const canvas = document.createElement('canvas');
		canvas.setAttribute('aria-hidden', 'true');
		canvas.width = Math.ceil(width * ratio);
		canvas.height = Math.ceil(height * ratio);
		canvas.style.width = `${width}px`;
		canvas.style.height = `${height}px`;
		const painter = canvas.getContext('2d');
		if (!painter) throw new Error('Spectrum canvas rendering is unavailable');
		painter.scale(ratio, ratio);
		const markId = group.getAttribute('data-ts-key');
		const radius = series.find((_, index) => markId === `series-${index}`)?.radius ?? 1;
		for (const point of context.scene.points) {
			if (point.markId !== markId) continue;
			painter.fillStyle = point.color;
			painter.beginPath();
			painter.arc(
				Math.round(point.x * 100) / 100,
				Math.round(point.y * 100) / 100,
				radius,
				0,
				Math.PI * 2
			);
			painter.fill();
		}
		layer.append(canvas);
		group.replaceChildren(layer);
	}
}
