<script lang="ts">
	import { SvelteSet } from 'svelte/reactivity';
	import { untrack } from 'svelte';
	import { mountChart } from '@tanstack/charts/dom';
	import { defineChart } from '@tanstack/charts/scene';
	import type { ChartPoint, ChartRenderContext } from '@tanstack/charts';
	import {
		buildChartDefinition,
		finitePoint,
		plotTooltip,
		type PlotOptions,
		type PlotPoint,
		type PlotSeries
	} from './chart-registry';
	import { paintLegendSwatch } from './legend-swatch';
	import { theme } from '#lib/stores/theme.svelte.ts';
	import { readSpectrumColors } from './chart-colors';
	import { paintSpectrumCloud } from './spectrum-cloud';
	import { renderCoverageSvg } from './coverage-marks';
	import { plotObservations, positionedScenePoints } from './chart-observations';
	import { createChartContract } from './chart-contract';
	import { MIN_DRAG_PIXELS, findNearestValueIndex } from './chart-utils';
	import { crosshairStore } from '#lib/stores/crosshair.ts';
	import { rangeSelection } from '#lib/stores/rangeSelection.svelte.ts';

	let {
		name,
		chartId,
		series,
		options,
		formatTooltip,
		onSelect,
		onRange,
		emptyCopy = 'No finite data for this selection.',
		retainEmptySurface = false
	}: {
		name: string;
		chartId?: string;
		series: PlotSeries[];
		options: PlotOptions;
		formatTooltip?: (points: readonly ChartPoint<PlotPoint, number, number>[]) => string;
		onSelect?: (point: PlotPoint) => void;
		onRange?: (start: PlotPoint, end: PlotPoint) => void;
		emptyCopy?: string;
		retainEmptySurface?: boolean;
	} = $props();
	const generatedId = $props.id();
	const hidden = new SvelteSet<string>();
	let context: ChartRenderContext<PlotPoint, number, number> | null = null;
	let contract: ReturnType<typeof createChartContract> | null = null;
	let contractSurface: HTMLElement | SVGElement | null = null;
	let cleanupRender = () => {};
	let rangeStart: number | null = null;
	let rangeEnd: number | null = null;
	let keyboardStart: PlotPoint | null = null;
	let focused: PlotPoint | null = null;
	let suppressSelect = false;
	let selection: HTMLDivElement | null = null;
	let externalCrosshair: HTMLDivElement | null = null;
	const hasData = $derived(series.some((item) => item.data.some(finitePoint)));
	const observationData = $derived(plotObservations(series, hidden));
	let observations: ReturnType<typeof plotObservations> | null = null;
	const definition = $derived.by(() => {
		const currentSeries = series;
		const currentOptions =
			options.colorDomain && typeof document !== 'undefined'
				? {
						...options,
						spectrumColors: readSpectrumColors(
							getComputedStyle(document.documentElement),
							theme.dark
						)
					}
				: options;
		const currentHidden = new Set(hidden);
		const formatter = formatTooltip;
		const normal = buildChartDefinition(
			currentSeries,
			{ ...currentOptions, compact: false },
			currentHidden,
			toggle,
			formatter
		);
		return defineChart({
			chart: ({ width }) =>
				currentOptions.compact && width < 560
					? buildChartDefinition(currentSeries, currentOptions, currentHidden, toggle, formatter)
					: normal,
			focus: normal.focus,
			tooltip: normal.tooltip,
			motion: false,
			svgAnimation: false,
			keyboard: true
		});
	});

	function toggle(visible: readonly string[]) {
		for (const item of series) {
			if (visible.includes(item.label)) hidden.delete(item.label);
			else hidden.add(item.label);
		}
	}

	function attachPlot(node: HTMLDivElement) {
		const hostOptions = () => ({
			definition,
			idPrefix: `atlantis-chart-${generatedId.replaceAll(/[^a-zA-Z0-9_-]/g, '')}`,
			ariaLabel: name,
			renderSvg: options.kind === 'coverage' ? renderCoverageSvg : undefined,
			onRender: publish,
			onFocusGroupChange: focusGroup,
			onSelect: select
		});
		let host: ReturnType<typeof mountChart<PlotPoint, number, number>> | null = null;
		let size: { width: number; height: number } | null = null;
		let currentOptions: ReturnType<typeof hostOptions>;
		const render = () => {
			if (!size) return;
			const current = { ...currentOptions, ...size };
			if (host) host.update(current);
			else host = mountChart(node, current);
		};
		$effect(() => {
			const current = hostOptions();
			const currentObservations = observationData;
			untrack(() => {
				observations = currentObservations;
				currentOptions = current;
				render();
			});
		});
		const observer = new ResizeObserver(([entry]) => {
			const { width, height } = entry.contentRect;
			if (width <= 0 || height <= 0 || (size?.width === width && size.height === height)) return;
			size = { width, height };
			render();
		});
		observer.observe(node);
		return () => {
			observer.disconnect();
			cleanupRender();
			contract?.destroy();
			host?.destroy();
			contract = null;
			contractSurface = null;
			context = null;
			observations = null;
			if (chartId && crosshairStore.sourceChartId === chartId) crosshairStore.clearHover();
			if (chartId && rangeSelection.selection?.sourceChartId === chartId) rangeSelection.clear();
		};
	}

	function publish(next: ChartRenderContext<PlotPoint, number, number>) {
		cleanupRender();
		paintSpectrumCloud(next, series);
		context = next;
		const surface = next.surface.element;
		if (!(surface instanceof HTMLElement || surface instanceof SVGElement))
			throw new TypeError('Expected an HTML or SVG chart surface');
		if (contractSurface !== surface) {
			contract?.destroy();
			contract = createChartContract(surface);
			contractSurface = surface;
		}
		const positioned = positionedScenePoints(next.scene);
		const buttons = Array.from(
			next.container.querySelectorAll<HTMLButtonElement>('[data-chart-legend-value]')
		);
		for (const button of buttons) {
			const item = series.find((item) => item.label === button.dataset.chartLegendValue);
			if (item) paintLegendSwatch(button, item, options.kind);
		}
		const bounds = surface.getBoundingClientRect();
		contract?.render({
			name,
			kind: options.kind === 'coverage' ? 'line' : (options.kind ?? 'line'),
			axes:
				options.compact && next.scene.width < 560
					? []
					: [options.xTitle, options.yTitle].filter(Boolean),
			series: series.map((item, index) => {
				const visible = !hidden.has(item.label);
				const points = positioned.get(`series-${index}`) ?? [];
				const values = visible
					? points.map((point) =>
							point.datum.value === undefined ? point.datum.y : point.datum.value
						)
					: item.data
							.filter((point) => {
								const x = next.scene.scales.x.map(point.x);
								return (
									finitePoint(point) &&
									Number.isFinite(x) &&
									x >= next.scene.chart.x &&
									x <= next.scene.chart.x + next.scene.chart.width
								);
							})
							.map((point) => (point.value === undefined ? point.y : point.value));
				const finite = values.filter(
					(value): value is number => typeof value === 'number' && Number.isFinite(value)
				);
				const button = buttons.find((button) => button.dataset.chartLegendValue === item.label);
				const box = button?.getBoundingClientRect();
				const target = points[0];
				const mark = next.svg.querySelector<SVGGraphicsElement>(
					`path[data-ts-key^="series-${index}-"], circle[data-ts-key^="series-${index}-"], circle[data-ts-key^="series-${index}:"]`
				);
				const appearance = mark ? getComputedStyle(mark) : null;
				const stroked = appearance?.stroke !== 'none';
				const color = appearance ? (stroked ? appearance.stroke : appearance.fill) : target?.color;
				const opacity = appearance
					? Number(appearance.opacity) *
						Number(stroked ? appearance.strokeOpacity : appearance.fillOpacity)
					: 1;
				return {
					label: item.label,
					visible,
					color,
					opacity: visible ? opacity : 0,
					count: finite.length,
					min: finite.length ? finite.reduce((min, value) => Math.min(min, value), Infinity) : null,
					max: finite.length
						? finite.reduce((max, value) => Math.max(max, value), -Infinity)
						: null,
					total: finite.reduce((sum, value) => sum + value, 0),
					point:
						visible && target
							? { x: target.x / next.scene.width, y: target.y / next.scene.height }
							: undefined,
					legend:
						box && bounds.width && bounds.height
							? {
									x: (box.x + box.width / 2 - bounds.x) / bounds.width,
									y: (box.y + box.height / 2 - bounds.y) / bounds.height
								}
							: undefined
				};
			})
		});
		const legend = next.container.querySelector('.ts-chart__interactive-legend');
		const syncLegendTargets = () => {
			const bounds = surface.getBoundingClientRect();
			for (const button of next.container.querySelectorAll<HTMLButtonElement>(
				'[data-chart-legend-value]'
			)) {
				const box = button.getBoundingClientRect();
				if (bounds.width && bounds.height)
					contract?.legend(button.dataset.chartLegendValue ?? '', {
						x: (box.x + box.width / 2 - bounds.x) / bounds.width,
						y: (box.y + box.height / 2 - bounds.y) / bounds.height
					});
			}
		};
		legend?.addEventListener('scroll', syncLegendTargets);
		selection = document.createElement('div');
		selection.className =
			'pointer-events-none absolute border border-muted-foreground/70 bg-muted-foreground/20';
		externalCrosshair = document.createElement('div');
		externalCrosshair.className =
			'pointer-events-none absolute border-l border-dashed border-current opacity-60';
		next.container.append(selection, externalCrosshair);
		const unsubscribeHover = crosshairStore.subscribe(() => paintExternalHover());
		const unsubscribeRange = rangeSelection.subscribe(() => paintSelection());
		next.container.addEventListener('mousedown', beginDrag, true);
		next.container.addEventListener('mousemove', moveDrag, true);
		window.addEventListener('mouseup', finishDrag, true);
		next.container.addEventListener('keydown', handleKey, true);
		next.container.addEventListener('click', selectAtPointer, true);
		cleanupRender = () => {
			legend?.removeEventListener('scroll', syncLegendTargets);
			unsubscribeHover();
			unsubscribeRange();
			next.container.removeEventListener('mousedown', beginDrag, true);
			next.container.removeEventListener('mousemove', moveDrag, true);
			window.removeEventListener('mouseup', finishDrag, true);
			next.container.removeEventListener('keydown', handleKey, true);
			next.container.removeEventListener('click', selectAtPointer, true);
			selection?.remove();
			externalCrosshair?.remove();
		};
	}

	function atPixel(x: number): PlotPoint | null {
		if (!context) return null;
		const value = context.scene.scales.x.invert?.(x);
		const index =
			typeof value === 'number'
				? findNearestValueIndex(observations?.timestamps ?? [], value)
				: null;
		return index === null ? null : (observations?.timeline[index] ?? null);
	}

	function pointerPosition(event: MouseEvent) {
		return context?.interaction.clientToScene(event.clientX, event.clientY) ?? null;
	}

	function selectAtPointer(event: MouseEvent) {
		if (
			!onSelect ||
			!context ||
			(event.target instanceof Element && event.target.closest('button'))
		)
			return;
		const position = pointerPosition(event);
		const area = context.scene.chart;
		if (
			!position ||
			position.x < area.x ||
			position.x > area.x + area.width ||
			position.y < area.y ||
			position.y > area.y + area.height
		)
			return;
		event.stopImmediatePropagation();
		if (suppressSelect) {
			suppressSelect = false;
			return;
		}
		const point = atPixel(position.x);
		if (point) onSelect(point);
	}

	function beginDrag(event: MouseEvent) {
		if (
			!onRange ||
			event.button !== 0 ||
			!context ||
			(event.target instanceof Element && event.target.closest('button'))
		)
			return;
		const position = pointerPosition(event);
		const area = context.scene.chart;
		if (
			!position ||
			position.x < area.x ||
			position.x > area.x + area.width ||
			position.y < area.y ||
			position.y > area.y + area.height
		)
			return;
		rangeStart = position.x;
		rangeEnd = position.x;
		suppressSelect = false;
	}

	function moveDrag(event: MouseEvent) {
		if (rangeStart === null || !context) return;
		const position = pointerPosition(event);
		if (!position) return;
		rangeEnd = Math.max(
			context.scene.chart.x,
			Math.min(context.scene.chart.x + context.scene.chart.width, position.x)
		);
		const first = atPixel(rangeStart);
		const last = atPixel(rangeEnd);
		if (first && last && chartId)
			rangeSelection.set({
				sourceChartId: chartId,
				startLabel: first.label ?? '',
				endLabel: last.label ?? ''
			});
		paintSelection();
	}

	function finishDrag() {
		if (rangeStart === null || rangeEnd === null) return;
		if (Math.abs(rangeEnd - rangeStart) >= MIN_DRAG_PIXELS) {
			suppressSelect = true;
			const first = atPixel(Math.min(rangeStart, rangeEnd));
			const last = atPixel(Math.max(rangeStart, rangeEnd));
			if (first && last) onRange?.(first, last);
		}
		rangeStart = null;
		rangeEnd = null;
		rangeSelection.clear();
		paintSelection();
	}

	function paintSelection() {
		if (!selection || !context) return;
		const mirrored = rangeSelection.selection;
		const from = observations?.labeledPoints.get(mirrored?.startLabel);
		const to = observations?.labeledPoints.get(mirrored?.endLabel);
		const left = rangeStart ?? (from ? context.scene.scales.x.map(from.x) : null);
		const right = rangeEnd ?? (to ? context.scene.scales.x.map(to.x) : null);
		selection.hidden = left === null || right === null || Math.abs(right - left) < MIN_DRAG_PIXELS;
		if (left !== null && right !== null)
			selection.style.cssText = `left:${Math.min(left, right)}px;top:${context.scene.chart.y}px;width:${Math.abs(right - left)}px;height:${context.scene.chart.height}px`;
	}

	function paintExternalHover() {
		if (!externalCrosshair || !context) return;
		const label = chartId ? crosshairStore.getExternalLabel(chartId) : null;
		const point = label ? observations?.labeledPoints.get(label) : null;
		externalCrosshair.hidden = !point;
		if (point)
			externalCrosshair.style.cssText = `left:${context.scene.scales.x.map(point.x)}px;top:${context.scene.chart.y}px;height:${context.scene.chart.height}px`;
	}

	function focusGroup(points: readonly ChartPoint<PlotPoint, number, number>[]) {
		focused = points[0]?.datum ?? null;
		contract?.tooltip(plotTooltip(points, formatTooltip));
		if (chartId) {
			if (focused?.label) crosshairStore.setHover(focused.label, chartId);
			else if (crosshairStore.sourceChartId === chartId) crosshairStore.clearHover();
		}
	}

	function select(point: ChartPoint<PlotPoint, number, number> | null) {
		if (suppressSelect) {
			suppressSelect = false;
			return;
		}
		if (point) onSelect?.(point.datum);
	}

	function handleKey(event: KeyboardEvent) {
		if (!onRange || !context || (event.target instanceof Element && event.target.closest('button')))
			return;
		if (event.key === 'Escape') {
			keyboardStart = null;
			rangeSelection.clear();
			paintSelection();
			return;
		}
		if (keyboardStart && event.key === 'Enter' && focused) {
			event.preventDefault();
			event.stopImmediatePropagation();
			const start = keyboardStart;
			keyboardStart = null;
			onRange(start.x <= focused.x ? start : focused, start.x <= focused.x ? focused : start);
			rangeSelection.clear();
			return;
		}
		if (!event.shiftKey || !['ArrowLeft', 'ArrowRight'].includes(event.key)) return;
		event.preventDefault();
		event.stopImmediatePropagation();
		const points = observations?.timeline ?? [];
		const index = Math.max(
			0,
			points.findIndex((point) => point.x === focused?.x)
		);
		keyboardStart ??= focused ?? points[index];
		const next =
			points[
				Math.max(0, Math.min(points.length - 1, index + (event.key === 'ArrowLeft' ? -1 : 1)))
			];
		const target = context.scene.points.find((point) => point.datum.x === next?.x);
		if (!target || !keyboardStart) return;
		context.interaction.setControlledFocus(target);
		focused = target.datum;
		if (chartId)
			rangeSelection.set({
				sourceChartId: chartId,
				startLabel: keyboardStart.label ?? '',
				endLabel: focused.label ?? ''
			});
		paintSelection();
	}
</script>

<div class="relative h-full w-full min-w-0">
	{#if hasData || retainEmptySurface}
		<div class="ts-chart-host h-full">
			<div class="ts-chart-surface h-full w-full" {@attach attachPlot}></div>
		</div>
		{#if !hasData}<p
				class="text-muted-foreground pointer-events-none absolute inset-0 flex items-center justify-center px-4 text-center text-sm"
			>
				{emptyCopy}
			</p>{/if}
		<p class="sr-only">
			Use arrow keys to explore values.
			{#if onSelect}Press Enter to open a bucket.{/if}
			{#if onRange}Shift and arrow keys select a time range; Enter applies it and Escape cancels.{/if}
		</p>
	{:else}
		<p
			class="text-muted-foreground flex h-full items-center justify-center px-4 text-center text-sm"
		>
			{emptyCopy}
		</p>
	{/if}
</div>

<style>
	:global(.ts-chart__interactive-legend) {
		overflow: auto;
		align-content: start;
		grid-auto-rows: 44px;
	}

	:global(.ts-chart-host) {
		--ts-chart-foreground: var(--chart-text-color);
		--ts-chart-grid: var(--chart-grid-color);
		--ts-chart-tooltip-background: var(--chart-tooltip-bg);
		--ts-chart-tooltip-color: var(--chart-tooltip-text-color);
		--ts-chart-tooltip-border: 1px solid var(--chart-tooltip-border-color);
	}
</style>
