import { defineChart } from '@tanstack/charts/scene';
import { lineY } from '@tanstack/charts/line';
import { areaY } from '@tanstack/charts/area';
import { rect } from '@tanstack/charts/rect';
import { dot } from '@tanstack/charts/dot';
import { decorative } from '@tanstack/charts/mark/decorative';
import { crosshair } from '@tanstack/charts/crosshair';
import { focusGroupX } from '@tanstack/charts/focus';
import { tooltip } from '@tanstack/charts/tooltip';
import { interactiveColorLegend } from '@tanstack/charts/legend';
import { controlledSignal } from '@tanstack/charts/interaction/signal';
import { scaleLinear } from '@tanstack/charts/scales/linear';
import { scaleSequential } from 'd3-scale';
import type { ChartMark, ChartPoint } from '@tanstack/charts';
import { temporalTicks } from './temporal-ticks';
import { temporalGrid } from './temporal-grid';
import { coverageLineRuns } from './coverage-line-style';
import type { ChartCoverage } from './chart-utils';

const spectrumTooltip: typeof tooltip = {
	...tooltip,
	create(context) {
		const instance = tooltip.create(context);
		return {
			...instance,
			paint(input) {
				instance.paint(input);
				const element = context.container.querySelector<HTMLElement>('.ts-chart-tooltip');
				if (element) {
					element.setAttribute('role', 'tooltip');
					element.style.whiteSpace = 'pre-line';
				}
			}
		};
	}
};

export type PlotPoint = {
	x: number;
	y: number | null;
	label?: string;
	coverage?: ChartCoverage;
	f?: number;
	sd?: number;
	x2?: number;
	value?: number | null;
	y0?: number;
	y2?: number | null;
};

export type PlotSeries = {
	label: string;
	color: string;
	data: PlotPoint[];
	dash?: string;
	radius?: number;
	strokeWidth?: number;
	pointFill?: string;
	pointStroke?: string;
	pointStrokeWidth?: number;
};

export type PlotAnnotation = {
	opacity?: number;
	strokeWidth?: number;
	data: PlotPoint[];
	color: string;
	dash?: string;
};

export type PlotOptions = {
	kind?: 'line' | 'stacked' | 'scatter' | 'coverage';
	xTitle: string;
	yTitle: string;
	xDomain?: readonly [number, number];
	yDomain?: readonly [number, number];
	xTicks?: readonly number[];
	yTicks?: readonly number[];
	xFormat?: (value: number) => string;
	yFormat?: (value: number) => string;
	zero?: boolean;
	legend?: boolean;
	compact?: boolean;
	annotations?: PlotAnnotation[];
	colorDomain?: readonly [number, number];
};

export function finitePoint(point: PlotPoint): boolean {
	return (
		Number.isFinite(point.x) &&
		typeof point.y === 'number' &&
		Number.isFinite(point.y) &&
		point.coverage?.state !== 'unknown'
	);
}

export function plotBounds(series: PlotSeries[], singletonPadding = 1): readonly [number, number] {
	let min = Infinity;
	let max = -Infinity;
	for (const item of series)
		for (const point of item.data) {
			if (!finitePoint(point)) continue;
			min = Math.min(min, point.x);
			max = Math.max(max, point.x2 ?? point.x);
		}
	if (!Number.isFinite(min)) return [0, 1];
	return min === max ? [min - singletonPadding / 2, max + singletonPadding / 2] : [min, max];
}

export function plotTooltip(
	points: readonly ChartPoint<PlotPoint, number, number>[],
	format?: (points: readonly ChartPoint<PlotPoint, number, number>[]) => string
): string {
	if (!points.length) return '';
	if (format) return format(points);
	return [
		points[0].datum.label ?? String(points[0].xValue),
		...points.map((point) => `${point.groupLabel}: ${point.datum.y?.toLocaleString()}`)
	].join('\n');
}

export function buildChartDefinition(
	series: PlotSeries[],
	options: PlotOptions,
	hidden: ReadonlySet<string>,
	onToggle: (labels: readonly string[]) => void,
	formatTooltip?: (points: readonly ChartPoint<PlotPoint, number, number>[]) => string
) {
	const marks: ChartMark<PlotPoint, number, number>[] = [];
	const cumulative = new Map<number, number>();
	for (const [index, item] of series.entries()) {
		const id = `series-${index}`;
		const channels = {
			id,
			x: 'x' as const,
			y: 'y' as const,
			color: () => item.label,
			z: () => item.label
		};
		let displayedData = item.data;
		if (options.kind === 'scatter') {
			marks.push(
				dot(item.data, {
					...channels,
					color: options.colorDomain ? (point) => point.f ?? 0 : channels.color,
					r: item.radius ?? 1
				})
			);
		} else if (options.kind === 'coverage') {
			const track = options.xDomain ?? plotBounds(series);
			marks.push(
				decorative(
					lineY(
						[
							{ x: track[0], y: index },
							{ x: track[1], y: index }
						],
						{ id: `${id}-track`, x: 'x', y: 'y', stroke: 'var(--chart-grid-color)', strokeWidth: 1 }
					)
				)
			);
			for (const state of ['complete', 'partial', 'unknown'] as const) {
				const data = item.data.filter(
					(point) =>
						Number.isFinite(point.x) && Number.isFinite(point.y) && point.coverage?.state === state
				);
				marks.push(
					rect(data, {
						...channels,
						id: `${id}-${state}`,
						x: 'x',
						x1: 'x',
						x2: (point) => point.x2 ?? point.x,
						y: 'y',
						y1: (point) => (point.y ?? 0) - 0.055,
						y2: (point) => (point.y ?? 0) + 0.055,
						inset: 0,
						fill: state === 'complete' ? 'rgb(16,185,129)' : 'transparent'
					})
				);
				if (state === 'partial')
					for (const [position, point] of data.entries()) {
						marks.push(
							decorative(
								lineY([point, { ...point, x: point.x2 ?? point.x }], {
									id: `${id}-coverage-dash-${position}`,
									x: 'x',
									y: 'y',
									stroke: 'rgb(245,158,11)',
									strokeWidth: 2,
									strokeDasharray: '5 4'
								})
							)
						);
					}
			}
		} else if (options.kind === 'stacked') {
			const data = item.data.map((point) => {
				const y0 = cumulative.get(point.x) ?? 0;
				const y2 = point.y === null ? null : y0 + (hidden.has(item.label) ? 0 : point.y);
				if (y2 !== null) cumulative.set(point.x, y2);
				return { ...point, y0, y2 };
			});
			displayedData = data;
			marks.push(areaY(data, { ...channels, y1: 'y0', y2: 'y2', fillOpacity: 0.6 }));
			for (const [runIndex, run] of coverageLineRuns(
				data,
				(point) => point.y2,
				(point) => point.coverage?.state === 'partial'
			).entries()) {
				marks.push(
					decorative(
						lineY(run.points, {
							...channels,
							id: `${id}-outline-${runIndex}`,
							y: 'y2',
							strokeDasharray: run.partial ? '6 4' : item.dash,
							strokeWidth: item.strokeWidth ?? 3
						})
					)
				);
			}
		} else {
			for (const [runIndex, run] of coverageLineRuns(
				item.data,
				(point) => (finitePoint(point) ? point.y : null),
				(point) => point.coverage?.state === 'partial'
			).entries()) {
				marks.push(
					lineY(run.points, {
						...channels,
						id: `${id}-run-${runIndex}`,
						strokeDasharray: run.partial ? '6 4' : item.dash,
						strokeWidth: item.strokeWidth ?? 3
					})
				);
			}
		}
		if (options.kind !== 'scatter' && options.kind !== 'coverage') {
			const isolated = displayedData.filter(
				(point, position, data) =>
					finitePoint(point) &&
					(point.coverage?.state === 'partial' ||
						item.radius ||
						(!finitePoint(data[position - 1] ?? { x: NaN, y: null }) &&
							!finitePoint(data[position + 1] ?? { x: NaN, y: null })))
			);
			marks.push(
				decorative(
					dot(isolated, {
						...channels,
						y: (point) => (options.kind === 'stacked' ? (point.y2 ?? null) : point.y),
						id: `${id}-dots`,
						r: item.radius ?? 3,
						fill: item.pointFill ?? 'var(--card)',
						stroke: item.pointStroke ?? item.color,
						strokeWidth: item.pointStrokeWidth ?? 2
					})
				)
			);
		}
	}
	for (const [index, annotation] of (options.annotations ?? []).entries()) {
		marks.push(
			decorative(
				lineY(annotation.data, {
					id: `annotation-${index}`,
					x: 'x',
					y: 'y',
					stroke: annotation.color,
					strokeDasharray: annotation.dash,
					strokeWidth: annotation.strokeWidth ?? 1.5,
					strokeOpacity: annotation.opacity ?? 1
				})
			)
		);
	}
	marks.push(
		crosshair({ y: false, stroke: 'currentColor', strokeOpacity: 0.65, strokeDasharray: '3 3' })
	);
	const xDomain = options.xDomain ?? plotBounds(series);
	let maxY = 0;
	let minY = 0;
	for (const item of series) {
		if (hidden.has(item.label)) continue;
		for (const point of item.data) {
			if (!finitePoint(point)) continue;
			minY = Math.min(minY, point.y ?? 0);
			maxY = Math.max(
				maxY,
				options.kind === 'stacked' ? (cumulative.get(point.x) ?? 0) : (point.y ?? 0)
			);
		}
	}
	const yScale = options.yDomain
		? scaleLinear().domain(options.yDomain)
		: options.zero === false
			? scaleLinear
			: scaleLinear().domain([minY, maxY === minY ? maxY + 1 : maxY]);
	const labels = series.map((item) => item.label);
	const formatX = options.xFormat;
	const timeAxis = Boolean(options.xTicks && formatX);
	const xTicks =
		options.xTicks && formatX
			? temporalTicks(options.xTicks, formatX, options.compact)
			: options.xTicks;
	if (timeAxis && options.kind !== 'coverage')
		marks.unshift(
			temporalGrid(
				options.xTicks ?? [],
				new Set(temporalTicks(options.xTicks ?? [], formatX ?? String))
			)
		);
	const legend = interactiveColorLegend({
		placement: 'top',
		itemWidth: 130,
		visible: controlledSignal<readonly string[]>(
			labels.filter((label) => !hidden.has(label)),
			onToggle
		)
	});
	return defineChart({
		marks,
		margin:
			options.kind === 'coverage'
				? { left: 120, right: 8, top: 4, bottom: 4 }
				: {
						left: options.compact ? 56 : 80,
						right: 18,
						bottom: options.compact ? 35 : timeAxis ? 110 : 62
					},
		clip: true,
		theme: {
			foreground: 'var(--chart-text-color)',
			muted: 'var(--chart-text-color)',
			grid: 'var(--chart-grid-color)'
		},
		scales: {
			x: {
				scale: scaleLinear().domain(xDomain),
				grid: options.kind === 'coverage' || timeAxis ? false : { strokeOpacity: 1 },
				axis:
					options.kind === 'coverage'
						? false
						: {
								label: options.compact ? undefined : options.xTitle,
								ticks: {
									...(xTicks ? { values: xTicks } : { count: options.compact ? 4 : 8 }),
									format: formatX
								},
								tickLabels: {
									rotate: timeAxis && !options.compact ? -45 : 0,
									thin: timeAxis ? false : { minGap: 8 }
								}
							}
			},
			y: {
				scale: yScale,
				nice: !options.yDomain,
				grid: options.kind === 'coverage' ? false : { strokeOpacity: 1 },
				axis: {
					label: options.compact ? undefined : options.yTitle,
					ticks: {
						...(options.yTicks ? { values: options.yTicks } : { count: options.compact ? 5 : 8 }),
						format: options.yFormat
					}
				}
			}
		},
		color: options.colorDomain
			? {
					scale: scaleSequential((value: number) => `hsl(${270 - value * 210}, 70%, 50%)`).domain(
						options.colorDomain
					)
				}
			: {
					domain: labels,
					range: series.map((item) => item.color),
					legend:
						options.legend === false || options.compact
							? undefined
							: {
									...legend,
									height: (count, context) => Math.min(108, legend.height(count, context))
								}
				},
		focus: options.kind === 'scatter' ? undefined : focusGroupX,
		tooltip: {
			use: options.kind === 'scatter' ? spectrumTooltip : tooltip,
			formatGroup: (points) => plotTooltip(points, formatTooltip),
			motion: false
		},
		motion: false,
		svgAnimation: false,
		keyboard: true
	});
}
