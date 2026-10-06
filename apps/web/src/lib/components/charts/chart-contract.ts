export type RenderedSeries = {
	label: string;
	visible: boolean;
	count: number;
	min: number | null;
	max: number | null;
	total: number;
	point?: { x: number; y: number };
	legend?: { x: number; y: number };
};

export type RenderedChart = {
	name: string;
	kind: string;
	axes: string[];
	series: RenderedSeries[];
};

export function createChartContract(surface: HTMLElement | SVGElement) {
	const summary = document.createElement('div');
	summary.className = 'sr-only';
	summary.dataset.testid = 'chart-render-state';
	summary.setAttribute('role', 'group');
	const tooltip = document.createElement('span');
	tooltip.dataset.testid = 'chart-tooltip';
	tooltip.hidden = true;
	let rendered = false;
	let description = '';
	const seriesEntries = new Map<string, HTMLElement>();
	function syncAttributes() {
		if (surface.dataset.testid !== 'chart-surface') surface.dataset.testid = 'chart-surface';
		if (surface.getAttribute('role') !== 'img') surface.setAttribute('role', 'img');
		if (rendered && surface.dataset.chartRendered !== 'true')
			surface.dataset.chartRendered = 'true';
		if (description && surface.getAttribute('aria-description') !== description)
			surface.setAttribute('aria-description', description);
	}
	syncAttributes();
	const observer = new MutationObserver(syncAttributes);
	observer.observe(surface, {
		attributes: true,
		attributeFilter: ['data-testid', 'data-chart-rendered', 'role', 'aria-description']
	});
	surface.after(summary);

	return {
		render(snapshot: RenderedChart) {
			const visible = snapshot.series.filter((series) => series.visible);
			const marks = visible.reduce((count, series) => count + series.count, 0);
			summary.dataset.state = marks > 0 ? 'ready' : 'empty';
			summary.dataset.kind = snapshot.kind;
			summary.dataset.seriesCount = String(visible.length);
			summary.dataset.markCount = String(marks);
			description = `${snapshot.name}: ${visible.length} series, ${marks} data points. ${snapshot.axes.join(', ')}.`;
			rendered = true;
			syncAttributes();
			summary.setAttribute('aria-label', 'Rendered chart summary');
			seriesEntries.clear();
			const entries = snapshot.series.map((series) => {
				const entry = document.createElement('span');
				entry.dataset.testid = 'chart-series';
				entry.dataset.visible = String(series.visible);
				entry.dataset.count = String(series.count);
				entry.dataset.min = String(series.min);
				entry.dataset.max = String(series.max);
				entry.dataset.total = String(series.total);
				if (series.point) {
					entry.dataset.pointX = String(series.point.x);
					entry.dataset.pointY = String(series.point.y);
				}
				if (series.legend) {
					entry.dataset.legendX = String(series.legend.x);
					entry.dataset.legendY = String(series.legend.y);
				}
				entry.textContent = series.label;
				seriesEntries.set(series.label, entry);
				return entry;
			});
			const axes = snapshot.axes.map((label) => {
				const axis = document.createElement('span');
				axis.dataset.testid = 'chart-axis';
				axis.textContent = label;
				return axis;
			});
			summary.replaceChildren(...axes, ...entries, tooltip);
		},
		legend(label: string, position: { x: number; y: number }) {
			const entry = seriesEntries.get(label);
			if (entry) {
				entry.dataset.legendX = String(position.x);
				entry.dataset.legendY = String(position.y);
			}
		},
		tooltip(text: string) {
			if (tooltip.textContent !== text) tooltip.textContent = text;
			tooltip.hidden = text.length === 0;
		},
		destroy() {
			observer.disconnect();
			seriesEntries.clear();
			summary.remove();
			delete surface.dataset.chartRendered;
			surface.removeAttribute('aria-description');
		}
	};
}
