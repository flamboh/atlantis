import type { ChartPoint, ChartScene } from '@tanstack/charts';
import { finitePoint, type PlotPoint, type PlotSeries } from './chart-registry';

export function plotTimeline(series: PlotSeries[], hidden: ReadonlySet<string>) {
	const unique = new Map<number, PlotPoint>();
	for (const item of series) {
		if (hidden.has(item.label)) continue;
		for (const point of item.data) if (finitePoint(point)) unique.set(point.x, point);
	}
	return [...unique.values()].sort((left, right) => left.x - right.x);
}

export function plotObservations(series: PlotSeries[], hidden: ReadonlySet<string>) {
	const timeline = plotTimeline(series, hidden);
	return {
		timeline,
		timestamps: timeline.map((point) => point.x),
		labeledPoints: new Map(timeline.map((point) => [point.label, point]))
	};
}

export function positionedScenePoints(scene: ChartScene<PlotPoint, number, number>) {
	const seen = new Set<PlotPoint>();
	const positioned = new Map<string, ChartPoint<PlotPoint, number, number>[]>();
	for (const point of scene.points) {
		if (
			seen.has(point.datum) ||
			!finitePoint(point.datum) ||
			!Number.isFinite(point.x) ||
			!Number.isFinite(point.y) ||
			point.x < scene.chart.x - 0.01 ||
			point.x > scene.chart.x + scene.chart.width + 0.01
		)
			continue;
		seen.add(point.datum);
		const markId = point.markId.replace(/-(complete|partial|run-\d+)$/, '');
		const points = positioned.get(markId) ?? [];
		points.push(point);
		positioned.set(markId, points);
	}
	return positioned;
}
