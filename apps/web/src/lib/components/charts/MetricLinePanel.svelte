<script lang="ts">
	import { onDestroy } from 'svelte';
	import type {
		ActiveElement,
		ChartConfiguration,
		ChartEvent,
		ScriptableLineSegmentContext
	} from 'chart.js';
	import { Chart } from './chart-registry';
	import { buildCoveragePointStyle } from './coverage-line-style';
	import type { GroupByOption } from '$lib/components/netflow/types';
	import type { IpGranularity } from '$lib/types/types';
	import {
		formatIpGranularityTick,
		formatTemporalBucketLabel,
		shouldHighlightIpGranularityGrid
	} from './ip-time-axis';
	import { theme } from '$lib/stores/theme.svelte';
	import { crosshairStore } from '$lib/stores/crosshair';
	import { rangeSelection } from '$lib/stores/rangeSelection.svelte';
	import { cancelDrawFrame, requestDrawFrame } from '$lib/utils/animation-frame';
	import { formatDateAsPSTDateString } from '$lib/utils/timezone';
	import {
		MIN_DRAG_PIXELS,
		Y_AXIS_WIDTH,
		beginRangeDrag,
		buildMirroredSelectionStyle,
		chooseAdaptiveGranularity,
		createRangeDragState,
		endRangeDrag,
		findTemporalDataBounds,
		generateSlugFromLabel,
		getSelectionLabels,
		groupByBucketDurationMs,
		indexFromPixelX,
		isCoverageSegmentDashed,
		parseClickedLabel,
		updateRangeDrag,
		type ChartCoverage
	} from './chart-utils';

	export type MetricLineSeries = {
		label: string;
		values: Array<number | null>;
		color: string;
		dash?: number[];
		coverage: ChartCoverage[];
	};

	type MetricLinePoint = {
		x: number;
		y: number | null;
		bucketStart: number;
		coverage: ChartCoverage;
	};

	type Props = {
		chartId: string;
		title: string;
		hideTitle?: boolean;
		yAxisTitle: string;
		bucketStarts: number[];
		granularity: IpGranularity;
		groupBy: GroupByOption;
		series: MetricLineSeries[];
		valueFormat?: 'duration' | 'decimal' | 'integer';
		onDrillDown?: (groupBy: GroupByOption, startDate: string, endDate: string) => void;
		onNavigateToFile?: (slug: string) => void;
	};

	const props: Props = $props();

	let canvas = $state<HTMLCanvasElement | null>(null);
	let chart: Chart<'line', MetricLinePoint[], string> | null = null;
	let rangeDrag = $state(createRangeDragState());
	let selectionLeft = $derived(Math.min(rangeDrag.dragStartX, rangeDrag.dragCurrentX));
	let selectionWidth = $derived(Math.abs(rangeDrag.dragStartX - rangeDrag.dragCurrentX));
	let mirroredRange = $derived(rangeSelection.selection);
	let pointerMoveFrame: number | null = null;
	let pendingPointerMoveEvent: MouseEvent | null = null;
	const hasData = $derived(
		props.series.some((series) => series.values.some((value) => typeof value === 'number'))
	);

	function formatValue(value: number): string {
		if (props.valueFormat === 'duration') {
			return value >= 1000 ? `${(value / 1000).toFixed(2)} s` : `${value.toFixed(1)} ms`;
		}
		if (props.valueFormat === 'integer') {
			return Math.round(value).toLocaleString();
		}
		return value.toFixed(2);
	}

	function colors() {
		const style = getComputedStyle(document.documentElement);
		return {
			text: style.getPropertyValue('--chart-text-color').trim(),
			grid: style.getPropertyValue('--chart-grid-color').trim(),
			gridHighlight: style.getPropertyValue('--chart-grid-highlight-color').trim(),
			tooltip: style.getPropertyValue('--chart-tooltip-bg').trim(),
			tooltipText: style.getPropertyValue('--chart-tooltip-text-color').trim(),
			tooltipBorder: style.getPropertyValue('--chart-tooltip-border-color').trim()
		};
	}

	function publishRangeSelection(startIndex: number, endIndex: number) {
		const labels = getSelectionLabels(chart, startIndex, endIndex);
		if (!labels) return;
		rangeSelection.set({ sourceChartId: props.chartId, ...labels });
	}

	function applyRangeDrilldown(startIndex: number, endIndex: number) {
		if (!chart?.data.labels || !props.onDrillDown) return;
		const labels = chart.data.labels as string[];
		const from = Math.max(0, Math.min(labels.length - 1, Math.min(startIndex, endIndex)));
		const to = Math.max(0, Math.min(labels.length - 1, Math.max(startIndex, endIndex)));
		const startLabel = labels[from];
		const endLabel = labels[to];
		if (!startLabel || !endLabel) return;

		const startDate = parseClickedLabel(startLabel, props.groupBy);
		const endBucketStart = parseClickedLabel(endLabel, props.groupBy);
		if (Number.isNaN(startDate.getTime()) || Number.isNaN(endBucketStart.getTime())) return;

		const endExclusive = new Date(
			endBucketStart.getTime() + groupByBucketDurationMs(props.groupBy)
		);
		const selectedRangeMs = endExclusive.getTime() - startDate.getTime();
		props.onDrillDown(
			chooseAdaptiveGranularity(selectedRangeMs),
			formatDateAsPSTDateString(startDate),
			formatDateAsPSTDateString(endExclusive)
		);
	}

	function handleRangeMouseDown(event: MouseEvent) {
		cancelPendingPointerMove();
		beginRangeDrag(rangeDrag, event, canvas, chart, publishRangeSelection);
	}

	function applyPendingPointerMove() {
		pointerMoveFrame = null;
		const event = pendingPointerMoveEvent;
		pendingPointerMoveEvent = null;
		if (event) updateRangeDrag(rangeDrag, event, canvas, chart, publishRangeSelection);
	}

	function cancelPendingPointerMove() {
		if (pointerMoveFrame !== null) {
			cancelDrawFrame(pointerMoveFrame);
			pointerMoveFrame = null;
		}
		pendingPointerMoveEvent = null;
	}

	function flushPendingPointerMove() {
		if (pointerMoveFrame === null) return;
		cancelDrawFrame(pointerMoveFrame);
		applyPendingPointerMove();
	}

	function handleRangeMouseMove(event: MouseEvent) {
		if (!rangeDrag.isDraggingRange) return;
		pendingPointerMoveEvent = event;
		if (pointerMoveFrame === null) {
			pointerMoveFrame = requestDrawFrame(applyPendingPointerMove);
		}
	}

	function finishRangeSelection() {
		flushPendingPointerMove();
		endRangeDrag(rangeDrag, chart, applyRangeDrilldown);
		rangeSelection.clear();
	}

	let mirroredSelectionStyle = $derived(
		buildMirroredSelectionStyle(chart, mirroredRange, props.chartId)
	);

	function handleChartClick(event: ChartEvent, activeElements: ActiveElement[]) {
		if (rangeDrag.suppressNextClick) {
			rangeDrag.suppressNextClick = false;
			return;
		}
		if (!chart?.data.labels) return;

		const index =
			typeof event.x === 'number'
				? indexFromPixelX(chart, event.x)
				: (activeElements[0]?.index ?? null);
		if (index === null) return;
		const label = chart.data.labels[index] as string | undefined;
		if (!label) return;

		if (props.groupBy === '5min') {
			const slug = generateSlugFromLabel(label, props.groupBy);
			if (slug) props.onNavigateToFile?.(slug);
			return;
		}

		const clickedDate = parseClickedLabel(label, props.groupBy);
		if (Number.isNaN(clickedDate.getTime()) || !props.onDrillDown) return;

		if (props.groupBy === 'date') {
			props.onDrillDown(
				'hour',
				formatDateAsPSTDateString(new Date(clickedDate.getTime() - 15 * 24 * 60 * 60 * 1000)),
				formatDateAsPSTDateString(new Date(clickedDate.getTime() + 16 * 24 * 60 * 60 * 1000))
			);
		} else if (props.groupBy === 'hour') {
			props.onDrillDown(
				'30min',
				formatDateAsPSTDateString(new Date(clickedDate.getTime() - 3 * 24 * 60 * 60 * 1000)),
				formatDateAsPSTDateString(new Date(clickedDate.getTime() + 4 * 24 * 60 * 60 * 1000))
			);
		} else {
			props.onDrillDown(
				'5min',
				formatDateAsPSTDateString(clickedDate),
				formatDateAsPSTDateString(new Date(clickedDate.getTime() + 24 * 60 * 60 * 1000))
			);
		}
	}

	function destroyChart() {
		crosshairStore.unregister(props.chartId);
		if (crosshairStore.sourceChartId === props.chartId) crosshairStore.clearHover();
		chart?.destroy();
		chart = null;
	}

	function renderChart() {
		if (!canvas || props.bucketStarts.length === 0 || props.series.length === 0 || !hasData) {
			destroyChart();
			return;
		}
		const indexedBuckets: Array<{ bucketStart: number; index: number }> = props.bucketStarts.map(
			(bucketStart, index) => ({ bucketStart, index })
		);
		const dataBounds = findTemporalDataBounds(
			indexedBuckets,
			(item) => item.bucketStart,
			(item) => props.series.some((series) => typeof series.values[item.index] === 'number')
		);
		if (!dataBounds) {
			destroyChart();
			return;
		}

		const palette = colors();
		const datasets = props.series.map((series) => {
			const data: MetricLinePoint[] = props.bucketStarts.map((bucketStart, index) => {
				const coverage = series.coverage[index] ?? {
					state: 'unknown' as const,
					observedUnits: 0,
					expectedUnits: 0
				};
				return {
					x: bucketStart,
					y: series.values[index] ?? null,
					bucketStart,
					coverage
				};
			});
			const hasPartialCoverage = data.some((point) => point.coverage.state === 'partial');
			const pointStyle = hasPartialCoverage
				? buildCoveragePointStyle(
						data,
						(point) => point.y,
						(point) => point.coverage,
						series.color
					)
				: null;
			return {
				label: series.label,
				data,
				borderColor: series.color,
				backgroundColor: series.color,
				borderDash: series.dash ?? [],
				pointRadius: 0,
				...(pointStyle ?? {}),
				pointHoverRadius: 4,
				spanGaps: false,
				...(hasPartialCoverage
					? {
							segment: {
								borderDash: (context: ScriptableLineSegmentContext) => {
									const left = context.p0 as unknown as {
										raw?: { coverage?: ChartCoverage };
									};
									const right = context.p1 as unknown as {
										raw?: { coverage?: ChartCoverage };
									};
									return isCoverageSegmentDashed(left.raw, right.raw)
										? [6, 4]
										: (series.dash ?? []);
								}
							}
						}
					: {}),
				parsing: false as const,
				tension: 0.25
			};
		});
		const labels = props.bucketStarts.map((bucketStart) =>
			formatTemporalBucketLabel(bucketStart, props.granularity)
		);

		const config: ChartConfiguration<'line', MetricLinePoint[], string> = {
			type: 'line',
			data: { labels, datasets },
			options: {
				responsive: true,
				maintainAspectRatio: false,
				animation: false,
				normalized: true,
				onClick: handleChartClick,
				interaction: { mode: 'index', intersect: false },
				plugins: {
					legend: { position: 'top', labels: { color: palette.text } },
					tooltip: {
						backgroundColor: palette.tooltip,
						titleColor: palette.tooltipText,
						bodyColor: palette.tooltipText,
						borderColor: palette.tooltipBorder,
						borderWidth: 1,
						callbacks: {
							label: (context: { dataset: { label?: string }; parsed: { y: number | null } }) =>
								`${context.dataset.label ?? ''}: ${context.parsed.y === null ? 'No data' : formatValue(context.parsed.y)}`
						}
					},
					verticalCrosshair: {
						enabled: true,
						line: { color: 'rgba(100, 100, 100, 0.8)', width: 1, dash: [3, 3] },
						tooltip: {
							enabled: true,
							delay: 500,
							backgroundColor: palette.tooltip,
							textColor: palette.tooltipText,
							borderColor: palette.tooltipBorder,
							borderWidth: 1,
							borderRadius: 4,
							padding: 8,
							fontSize: 12,
							fontFamily: 'system-ui, sans-serif'
						},
						sync: {
							onHover: (label: string | null) => crosshairStore.setHover(label, props.chartId),
							getExternalLabel: () => crosshairStore.getExternalLabel(props.chartId)
						}
					}
				} as never,
				scales: {
					x: {
						type: 'linear',
						min: dataBounds.min,
						max: dataBounds.max,
						title: {
							display: true,
							text: `Time (${props.granularity})`,
							color: palette.text
						},
						ticks: {
							color: palette.text,
							autoSkip: false,
							maxRotation: 45,
							minRotation: 45,
							sampleSize: 12,
							callback: (value: string | number) =>
								formatIpGranularityTick(Number(value), props.granularity, 0)
						},
						grid: {
							color: (context: { tick?: { value?: number } }) =>
								shouldHighlightIpGranularityGrid(
									Number(context.tick?.value ?? 0),
									props.granularity,
									0
								)
									? palette.grid
									: palette.gridHighlight
						}
					},
					y: {
						beginAtZero: true,
						afterFit(axis) {
							axis.width = Y_AXIS_WIDTH;
						},
						title: { display: true, text: props.yAxisTitle, color: palette.text },
						ticks: {
							color: palette.text,
							callback: (value: string | number) => formatValue(Number(value))
						},
						grid: { color: palette.grid }
					}
				}
			}
		};
		if (chart && chart.canvas === canvas) {
			chart.data = config.data;
			chart.options = config.options as never;
			chart.update('none');
			return;
		}

		destroyChart();
		chart = new Chart(canvas, config);
		crosshairStore.register(props.chartId, chart);
	}

	$effect(() => {
		void theme.dark;
		renderChart();
	});

	onDestroy(() => {
		cancelPendingPointerMove();
		if (mirroredRange?.sourceChartId === props.chartId) rangeSelection.clear();
		destroyChart();
	});
</script>

<section class="flex h-full min-h-0 min-w-0 flex-col" aria-label={props.title}>
	{#if !props.hideTitle}
		<h3 class="text-foreground px-3 pt-3 text-sm font-semibold">{props.title}</h3>
	{/if}
	{#if !hasData}
		<div class="text-muted-foreground flex flex-1 items-center justify-center text-sm">
			No data for this metric
		</div>
	{:else}
		<div
			class="relative min-h-0 flex-1"
			role="presentation"
			onmousedown={handleRangeMouseDown}
			onmousemove={handleRangeMouseMove}
			onmouseup={finishRangeSelection}
			onmouseleave={finishRangeSelection}
		>
			<canvas bind:this={canvas} class="h-full w-full" aria-label={`${props.title} time series`}
			></canvas>
			{#if rangeDrag.isDraggingRange && selectionWidth >= MIN_DRAG_PIXELS}
				<div
					class="border-muted-foreground/70 bg-muted-foreground/20 pointer-events-none absolute border"
					style={`left:${selectionLeft}px; width:${selectionWidth}px; top:${rangeDrag.selectionTop}px; height:${rangeDrag.selectionHeight}px;`}
				></div>
			{/if}
			{#if !rangeDrag.isDraggingRange && mirroredSelectionStyle !== null}
				<div
					class="border-muted-foreground/70 bg-muted-foreground/20 pointer-events-none absolute border"
					style={mirroredSelectionStyle}
				></div>
			{/if}
		</div>
		<p class="sr-only">
			{props.series.length} series across {props.bucketStarts.length} time buckets.
		</p>
	{/if}
</section>
