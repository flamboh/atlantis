import type { PlotOptions, PlotSeries } from './chart-registry';

export function paintLegendSwatch(
	button: HTMLButtonElement,
	series: PlotSeries,
	kind: PlotOptions['kind']
) {
	const swatch = button.querySelector<HTMLElement>('[data-chart-legend-swatch]');
	if (!swatch) return;
	const document = button.ownerDocument;
	const ns = 'http://www.w3.org/2000/svg';
	const svg = document.createElementNS(ns, 'svg');
	svg.setAttribute('width', '28');
	svg.setAttribute('height', '12');
	svg.setAttribute('aria-hidden', 'true');
	const mark = document.createElementNS(ns, kind === 'stacked' ? 'rect' : 'path');
	if (kind === 'stacked') {
		mark.setAttribute('x', '1');
		mark.setAttribute('y', '1');
		mark.setAttribute('width', '26');
		mark.setAttribute('height', '10');
		mark.setAttribute('fill', series.color);
		mark.setAttribute('fill-opacity', '0.6');
	} else {
		mark.setAttribute('d', 'M0,6H28');
		mark.setAttribute('fill', 'none');
	}
	mark.setAttribute('stroke', series.color);
	mark.setAttribute('stroke-width', String(series.strokeWidth ?? 3));
	if (series.dash) mark.setAttribute('stroke-dasharray', series.dash);
	svg.append(mark);
	if (series.radius) {
		const point = document.createElementNS(ns, 'circle');
		point.setAttribute('cx', '14');
		point.setAttribute('cy', '6');
		point.setAttribute('r', String(series.radius));
		point.setAttribute('fill', series.pointFill ?? 'var(--card)');
		point.setAttribute('stroke', series.pointStroke ?? series.color);
		point.setAttribute('stroke-width', String(series.pointStrokeWidth ?? 2));
		svg.append(point);
	}
	swatch.style.width = '28px';
	swatch.style.height = '12px';
	swatch.style.border = '0';
	swatch.style.borderRadius = '0';
	swatch.style.background = 'transparent';
	swatch.style.opacity = button.getAttribute('aria-pressed') === 'false' ? '0.4' : '1';
	swatch.replaceChildren(svg);
}
