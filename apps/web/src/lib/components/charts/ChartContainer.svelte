<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import type { TooltipItem } from 'chart.js';
	import { getRelativePosition } from 'chart.js/helpers';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { Chart } from './chart-registry';
	import { crosshairStore } from '$lib/stores/crosshair';
	import { rangeSelectionStore, type RangeSelectionState } from '$lib/stores/rangeSelection';
	import { theme } from '$lib/stores/theme.svelte';
	import { SvelteSet } from 'svelte/reactivity';
	import { NETFLOW_DATA_OPTION_FIELDS } from '$lib/components/netflow/constants';
	import {
		formatLabels,
		getXAxisTitle,
		parseClickedLabel,
		generateSlugFromLabel,
		Y_AXIS_WIDTH,
		MIN_DRAG_PIXELS,
		groupByBucketDurationMs,
		chooseAdaptiveGranularity,
		createRangeDragState,
		getSelectionLabels,
		beginRangeDrag,
		updateRangeDrag,
		endRangeDrag,
		buildMirroredSelectionStyle
	} from './chart-utils';
	import {
		parseLabelToPSTComponents,
		formatDateAsPSTDateString,
		epochToPSTComponents,
		getWeekdayName,
		type PSTDateComponents
	} from '$lib/utils/timezone';
	import type {
		NetflowDataPoint,
		GroupByOption,
		ChartTypeOption,
		DataOption,
		ClickedElement,
		ChartConfig,
		ChartDataset
	} from '$lib/components/netflow/types.ts';

	interface Props {
		results: NetflowDataPoint[];
		groupBy: GroupByOption;
		chartType: ChartTypeOption;
		dataOptions: DataOption[];
		onDrillDown?: (newGroupBy: GroupByOption, newStartDate: string, newEndDate: string) => void;
		onNavigateToFile?: (slug: string) => void;
	}

	let { results, groupBy, chartType, dataOptions, onDrillDown, onNavigateToFile }: Props = $props();

	const CHART_ID = 'netflow';
	const COMPACT_CHART_WIDTH = 560;

	let chartCanvas: HTMLCanvasElement;
	let chart: Chart | null = null;
	let resizeObserver: ResizeObserver | null = null;
	let compactChartMode: boolean | null = null;
	let rangeDrag = $state(createRangeDragState());
	let selectionLeft = $derived(Math.min(rangeDrag.dragStartX, rangeDrag.dragCurrentX));
	let selectionWidth = $derived(Math.abs(rangeDrag.dragCurrentX - rangeDrag.dragStartX));
	let mirroredRange = $state<RangeSelectionState | null>(null);

	type MetricFamily = 'flows' | 'packets' | 'bytes';

	$effect(() => {
		const unsubscribe = rangeSelectionStore.subscribe((value) => {
			mirroredRange = value;
		});
		return unsubscribe;
	});

	function publishRangeSelection(startIndex: number, endIndex: number) {
		const labels = getSelectionLabels(chart, startIndex, endIndex);
		if (!labels) return;
		rangeSelectionStore.set({ sourceChartId: CHART_ID, ...labels });
	}

	function applyRangeDrilldown(startIndex: number, endIndex: number) {
		if (!chart?.data.labels || !onDrillDown) return;
		const labels = chart.data.labels as string[];
		if (labels.length === 0) return;
		const safeStart = Math.max(0, Math.min(labels.length - 1, startIndex));
		const safeEnd = Math.max(0, Math.min(labels.length - 1, endIndex));
		const from = Math.min(safeStart, safeEnd);
		const to = Math.max(safeStart, safeEnd);
		const startLabel = labels[from];
		const endLabel = labels[to];
		if (!startLabel || !endLabel) return;

		const startDate = parseClickedLabel(startLabel, groupBy);
		const endBucketStart = parseClickedLabel(endLabel, groupBy);
		if (Number.isNaN(startDate.getTime()) || Number.isNaN(endBucketStart.getTime())) return;

		const endExclusive = new Date(endBucketStart.getTime() + groupByBucketDurationMs(groupBy));
		const selectedRangeMs = endExclusive.getTime() - startDate.getTime();
		const nextGroupBy = chooseAdaptiveGranularity(selectedRangeMs);
		onDrillDown(
			nextGroupBy,
			formatDateAsPSTDateString(startDate),
			formatDateAsPSTDateString(endExclusive)
		);
	}

	function handleRangeMouseDown(event: MouseEvent) {
		beginRangeDrag(rangeDrag, event, chartCanvas, chart, publishRangeSelection);
	}

	function handleRangeMouseMove(event: MouseEvent) {
		updateRangeDrag(rangeDrag, event, chartCanvas, chart, publishRangeSelection);
	}

	function finishRangeSelection() {
		endRangeDrag(rangeDrag, chart, applyRangeDrilldown);
		rangeSelectionStore.set(null);
	}

	let mirroredSelectionStyle = $derived(
		buildMirroredSelectionStyle(chart, mirroredRange, CHART_ID)
	);

	function formatTickLabel(
		pst: PSTDateComponents | null,
		currentGroupBy: GroupByOption,
		index: number
	): string {
		if (!pst) {
			return '';
		}
		const weekday = getWeekdayName(pst.dayOfWeek);
		const month = pst.month;
		const day = pst.day;
		const hours = pst.hours;
		const minutes = pst.minutes;

		if (currentGroupBy === 'date') {
			return pst.dayOfWeek === 1 ? `${weekday} ${month}/${day}` : '';
		}

		if (currentGroupBy === 'hour') {
			return hours === 0 ? `${weekday} ${month}/${day}` : '';
		}

		if (currentGroupBy === '30min') {
			if (minutes === 0 && (hours === 0 || hours === 12)) {
				return `${weekday} ${month}/${day} ${hours.toString().padStart(2, '0')}:00`;
			}
			return '';
		}

		if (currentGroupBy === '5min') {
			if (minutes === 0) {
				return `${weekday} ${month}/${day} ${hours.toString().padStart(2, '0')}:00`;
			}
			return '';
		}

		return index === 0 ? `${weekday} ${month}/${day}` : '';
	}

	function shouldHighlightTick(
		pst: PSTDateComponents | null,
		currentGroupBy: GroupByOption,
		index: number
	): boolean {
		if (!pst) {
			return index === 0;
		}
		const hours = pst.hours;
		const minutes = pst.minutes;

		if (currentGroupBy === 'date') {
			return pst.dayOfWeek === 1;
		}
		if (currentGroupBy === 'hour') {
			return hours === 0;
		}
		if (currentGroupBy === '30min') {
			return minutes === 0 && (hours === 0 || hours === 12);
		}
		if (currentGroupBy === '5min') {
			return minutes === 0;
		}
		return index === 0;
	}

	function getClickedElement(
		activeElements: { datasetIndex: number; index: number }[]
	): ClickedElement | null {
		if (!chart) {
			return null;
		}
		if (activeElements.length > 0) {
			const element = activeElements[0];
			const datasetIndex = element.datasetIndex;
			const index = element.index;
			const dataset = chart.data.datasets[datasetIndex] as ChartDataset;
			const label = chart.data.labels?.[index] as string;
			const value = dataset.data[index] as number;
			return {
				dataset: {
					label: dataset.label,
					data: dataset.data,
					backgroundColor: dataset.backgroundColor as string,
					borderColor: dataset.borderColor as string
				},
				label,
				value,
				datasetIndex,
				index
			};
		}
		return null;
	}

	function getMetricFamily(label: string): MetricFamily | null {
		const normalized = label.toLowerCase();
		if (normalized.includes('flow')) return 'flows';
		if (normalized.includes('packet')) return 'packets';
		if (normalized.includes('byte')) return 'bytes';
		return null;
	}

	function formatMetricValue(value: number, family: MetricFamily): string {
		if (family === 'bytes') {
			if (value >= 1024 ** 5) return `${(value / 1024 ** 5).toFixed(1)} PB`;
			if (value >= 1024 ** 4) return `${(value / 1024 ** 4).toFixed(1)} TB`;
			if (value >= 1024 ** 3) return `${(value / 1024 ** 3).toFixed(1)} GB`;
			if (value >= 1024 ** 2) return `${(value / 1024 ** 2).toFixed(1)} MB`;
			if (value >= 1024) return `${(value / 1024).toFixed(1)} KB`;
			return `${value.toLocaleString()} bytes`;
		}

		return value.toLocaleString();
	}

	function getTooltipTotalLines(items: TooltipItem<'line'>[]): string[] {
		const dataIndex = items[0]?.dataIndex;
		if (dataIndex == null) {
			return [];
		}

		const bucket = results[dataIndex];
		if (!bucket) {
			return [];
		}

		const visibleFamilies = new SvelteSet<MetricFamily>();
		for (const item of items) {
			const family = getMetricFamily(item.dataset.label ?? '');
			if (family) {
				visibleFamilies.add(family);
			}
		}

		const lines: string[] = [];
		if (visibleFamilies.has('flows')) {
			lines.push(`Total Flows: ${formatMetricValue(bucket.flows, 'flows')}`);
		}
		if (visibleFamilies.has('packets')) {
			lines.push(`Total Packets: ${formatMetricValue(bucket.packets, 'packets')}`);
		}
		if (visibleFamilies.has('bytes')) {
			lines.push(`Total Bytes: ${formatMetricValue(bucket.bytes, 'bytes')}`);
		}

		return lines;
	}

	function handleChartClick(
		e: MouseEvent,
		activeElements: { datasetIndex: number; index: number }[]
	) {
		if (rangeDrag.suppressNextClick) {
			rangeDrag.suppressNextClick = false;
			return;
		}
		if (!chart) {
			return;
		}
		// Always get the canvas position and convert to data values
		const canvasPosition = getRelativePosition(e, chart);
		const dataX = chart.scales.x.getValueForPixel(canvasPosition.x);

		// Convert the x-axis data value to the appropriate date based on current groupBy
		let clickedDate: Date;

		if (typeof dataX === 'number') {
			// dataX is the index in the labels array
			const labelIndex = Math.round(dataX);
			const labels = chart.data.labels;

			if (labels && labelIndex >= 0 && labelIndex < labels.length) {
				const clickedLabel = labels[labelIndex] as string;
				clickedDate = parseClickedLabel(clickedLabel, groupBy);
			} else {
				// Handle clicks outside the data range
				if (labels && labels.length > 0) {
					let targetLabel: string;
					if (labelIndex < 0) {
						targetLabel = labels[0] as string;
					} else {
						targetLabel = labels[labels.length - 1] as string;
					}
					clickedDate = parseClickedLabel(targetLabel, groupBy);
				} else {
					return;
				}
			}
		} else {
			return;
		}

		const clickedElement = getClickedElement(activeElements);

		if (groupBy === 'date') {
			const startOfMonth = new Date(clickedDate.getTime() - 15 * 24 * 60 * 60 * 1000);
			const endOfMonth = new Date(clickedDate.getTime() + 16 * 24 * 60 * 60 * 1000);
			const startDateStr = formatDateAsPSTDateString(startOfMonth);
			const endDateStr = formatDateAsPSTDateString(endOfMonth);
			onDrillDown?.('hour', startDateStr, endDateStr);
		} else if (groupBy === 'hour') {
			const startOfWeek = new Date(clickedDate.getTime() - 3 * 24 * 60 * 60 * 1000);
			const endOfWeek = new Date(clickedDate.getTime() + 4 * 24 * 60 * 60 * 1000);
			const startDateStr = formatDateAsPSTDateString(startOfWeek);
			const endDateStr = formatDateAsPSTDateString(endOfWeek);
			onDrillDown?.('30min', startDateStr, endDateStr);
		} else if (groupBy === '30min') {
			const endDate = new Date(clickedDate.getTime() + 24 * 60 * 60 * 1000);
			const startDateStr = formatDateAsPSTDateString(clickedDate);
			const endDateStr = formatDateAsPSTDateString(endDate);
			onDrillDown?.('5min', startDateStr, endDateStr);
		} else if (groupBy === '5min') {
			// For 5min level, we need to create a slug from the clicked date
			// Convert the date back to a label format that generateSlugFromLabel expects
			let labelForSlug: string;
			if (clickedElement) {
				labelForSlug = clickedElement.label;
			} else {
				// Format the date to match the label format for 5min grouping using PST
				const pst = epochToPSTComponents(Math.floor(clickedDate.getTime() / 1000));
				const year = pst.year;
				const month = String(pst.month).padStart(2, '0');
				const day = String(pst.day).padStart(2, '0');
				const hour = String(pst.hours).padStart(2, '0');
				const minute = String(pst.minutes).padStart(2, '0');
				labelForSlug = `${year}-${month}-${day} ${hour}:${minute}`;
			}

			const slug = generateSlugFromLabel(labelForSlug, groupBy);
			if (onNavigateToFile) {
				onNavigateToFile(slug);
			} else {
				goto(resolve(`/netflow/files/${slug}`));
			}
		}
	}

	function parseLabelToPST(label: string | undefined): PSTDateComponents | null {
		if (!label) return null;
		return parseLabelToPSTComponents(label);
	}

	function getChartColors() {
		const style = getComputedStyle(document.documentElement);
		return {
			textColor: style.getPropertyValue('--chart-text-color').trim(),
			gridColor: style.getPropertyValue('--chart-grid-color').trim(),
			gridHighlightColor: style.getPropertyValue('--chart-grid-highlight-color').trim(),
			tooltipBackgroundColor: style.getPropertyValue('--chart-tooltip-bg').trim(),
			tooltipTextColor: style.getPropertyValue('--chart-tooltip-text-color').trim(),
			tooltipBorderColor: style.getPropertyValue('--chart-tooltip-border-color').trim()
		};
	}

	function isCompactChart(): boolean {
		const width = chartCanvas.parentElement?.clientWidth ?? chartCanvas.clientWidth;
		return width < COMPACT_CHART_WIDTH;
	}

	function getLabelPSTFromLabels(
		labels: (string | number | null | undefined)[],
		idx: number
	): PSTDateComponents | null {
		const label = labels[idx];
		return typeof label === 'string' ? parseLabelToPST(label) : null;
	}

	function createChartConfig(): ChartConfig {
		const {
			textColor,
			gridColor,
			gridHighlightColor,
			tooltipBackgroundColor,
			tooltipTextColor,
			tooltipBorderColor
		} = getChartColors();
		const labels = formatLabels(results, groupBy);
		const getLabelPST = (idx: number): PSTDateComponents | null =>
			getLabelPSTFromLabels(labels, idx);
		const xAxisTitle = getXAxisTitle(groupBy);
		const compactChart = isCompactChart();

		// Use manual chart type selection - matches original logic
		const isStackedChart = chartType === 'stacked';

		// Original predefined colors
		const predefinedColors = [
			'rgb(75, 192, 192)',
			'rgb(255, 99, 132)',
			'rgb(54, 162, 235)',
			'rgb(255, 206, 86)',
			'rgb(153, 102, 255)',
			'rgb(255, 159, 64)',
			'rgb(255, 99, 71)',
			'rgb(0, 206, 209)',
			'rgb(60, 179, 113)',
			'rgb(218, 112, 214)',
			'rgb(255, 215, 0)',
			'rgb(128, 0, 128)',
			'rgb(0, 128, 128)',
			'rgb(255, 0, 255)',
			'rgb(0, 255, 0)',
			'rgb(128, 128, 0)',
			'rgb(0, 0, 128)',
			'rgb(255, 140, 0)',
			'rgb(34, 139, 34)',
			'rgb(139, 0, 0)'
		];

		// Parse data from results - matches original parsing logic
		const datasets: ChartDataset[] = [];
		let colorIndex = 0;

		for (const option of dataOptions) {
			if (option.checked) {
				const field = NETFLOW_DATA_OPTION_FIELDS[option.index];
				if (!field) {
					continue;
				}
				const data = results.map((item) => item[field]);
				const color = predefinedColors[colorIndex % predefinedColors.length];
				colorIndex++;

				datasets.push({
					label: option.label,
					data: data,
					borderColor: color,
					backgroundColor: isStackedChart
						? color.replace('rgb', 'rgba').replace(')', ', 0.6)')
						: color,
					fill: isStackedChart ? 'origin' : false,
					tension: 0.1,
					radius: 0,
					hitRadius: 2,
					hoverRadius: 5
				});
			}
		}

		// Check if all selected metrics are bytes
		const selectedOptions = dataOptions.filter((o) => o.checked);
		const allAreBytesMetrics = selectedOptions.every((o) => o.label.includes('Bytes'));
		const formatAxisValue = (value: string | number) => {
			const num = Number(value);

			if (allAreBytesMetrics) {
				if (num >= Math.pow(1024, 5)) return (num / Math.pow(1024, 5)).toFixed(1) + 'PB';
				if (num >= Math.pow(1024, 4)) return (num / Math.pow(1024, 4)).toFixed(1) + 'TB';
				if (num >= Math.pow(1024, 3)) return (num / Math.pow(1024, 3)).toFixed(1) + 'GB';
				if (num >= Math.pow(1024, 2)) return (num / Math.pow(1024, 2)).toFixed(1) + 'MB';
				if (num >= 1024) return (num / 1024).toFixed(1) + 'KB';
				return num.toString() + ' bytes';
			}

			if (num >= 1e15) return (num / 1e15).toFixed(1) + 'Q';
			if (num >= 1e12) return (num / 1e12).toFixed(1) + 'T';
			if (num >= 1e9) return (num / 1e9).toFixed(1) + 'B';
			if (num >= 1e6) return (num / 1e6).toFixed(1) + 'M';
			if (num >= 1e3) return (num / 1e3).toFixed(1) + 'K';
			return num.toString();
		};

		// Original scales configuration
		const scales: Record<string, object> = {
			x: {
				title: {
					display: !compactChart,
					text: xAxisTitle,
					color: textColor
				},
				ticks: {
					color: textColor,
					autoSkip: compactChart,
					maxRotation: compactChart ? 0 : 45,
					maxTicksLimit: compactChart ? 4 : undefined,
					minRotation: compactChart ? 0 : 45,
					sampleSize: 12,
					callback: (_val: string | number, idx: number) =>
						formatTickLabel(getLabelPST(Number(idx)), groupBy, Number(idx))
				},
				grid: {
					color: (ctx: { index?: number; tick?: { index?: number } }) => {
						const tickIndex = ctx.index ?? ctx.tick?.index ?? 0;
						const safeIndex = Number.isFinite(Number(tickIndex)) ? Number(tickIndex) : 0;
						return shouldHighlightTick(getLabelPST(safeIndex), groupBy, safeIndex)
							? gridColor
							: gridHighlightColor;
					}
				}
			},
			y: isStackedChart
				? {
						display: true,
						type: 'linear',
						stacked: true,
						beginAtZero: true,
						afterFit(axis: { width: number }) {
							axis.width = compactChart ? 56 : Y_AXIS_WIDTH;
						},
						title: {
							display: !compactChart,
							text: 'Value',
							color: textColor
						},
						ticks: {
							color: textColor,
							callback: formatAxisValue
						},
						grid: {
							color: gridColor
						}
					}
				: {
						display: true,
						type: 'linear',
						beginAtZero: true,
						afterFit(axis: { width: number }) {
							axis.width = compactChart ? 56 : Y_AXIS_WIDTH;
						},
						title: {
							display: !compactChart,
							text: 'Value',
							color: textColor
						},
						ticks: {
							color: textColor,
							callback: formatAxisValue
						},
						grid: {
							color: gridColor
						}
					}
		};

		return {
			type: 'line',
			data: {
				labels,
				datasets
			},
			options: {
				onClick: handleChartClick,
				responsive: true,
				maintainAspectRatio: false,
				animation: false,
				interaction: {
					mode: 'index',
					intersect: false
				},
				scales: scales,
				plugins: {
					legend: {
						display: !compactChart,
						position: 'top' as const,
						labels: { color: textColor }
					},
					tooltip: {
						mode: 'index',
						intersect: false,
						backgroundColor: tooltipBackgroundColor,
						titleColor: tooltipTextColor,
						bodyColor: tooltipTextColor,
						footerColor: tooltipTextColor,
						borderColor: tooltipBorderColor,
						borderWidth: 1,
						callbacks: {
							label: (context: TooltipItem<'line'>) => {
								const family = getMetricFamily(context.dataset.label ?? '') ?? 'flows';
								const value = Number(context.parsed.y ?? 0);
								return `${context.dataset.label}: ${formatMetricValue(value, family)}`;
							},
							footer: (items: TooltipItem<'line'>[]) => getTooltipTotalLines(items)
						}
					},
					verticalCrosshair: {
						enabled: true,
						line: {
							color: 'rgba(100, 100, 100, 0.8)',
							width: 1,
							dash: [3, 3]
						},
						tooltip: {
							enabled: true,
							delay: 500,
							backgroundColor: tooltipBackgroundColor,
							textColor: tooltipTextColor,
							borderColor: tooltipBorderColor,
							borderWidth: 1,
							borderRadius: 4,
							padding: 8,
							fontSize: 12,
							fontFamily: 'system-ui, sans-serif'
						},
						sync: {
							onHover: (label: string | null) => crosshairStore.setHover(label, CHART_ID),
							getExternalLabel: () => crosshairStore.getExternalLabel(CHART_ID)
						}
					}
				} as Record<string, object>
			}
		};
	}

	onMount(() => {
		const {
			textColor,
			gridColor,
			gridHighlightColor,
			tooltipBackgroundColor,
			tooltipTextColor,
			tooltipBorderColor
		} = getChartColors();

		// Initialize empty chart (matches original)
		chart = new Chart(chartCanvas, {
			type: 'line',
			data: {
				labels: [],
				datasets: [] // Start with no datasets
			},
			options: {
				onClick: handleChartClick,
				responsive: true,
				maintainAspectRatio: false,
				interaction: {
					mode: 'index',
					intersect: false
				},
				scales: {
					x: {
						title: {
							display: true,
							text: 'Date',
							color: textColor
						},
						ticks: {
							color: textColor,
							autoSkip: false,
							maxRotation: 45,
							minRotation: 45,
							sampleSize: 12,
							callback: (_val: string | number, idx: number) =>
								formatTickLabel(
									getLabelPSTFromLabels(
										(chart?.data.labels as (string | number | null | undefined)[] | undefined) ??
											[],
										Number(idx)
									),
									groupBy,
									Number(idx)
								)
						},
						grid: {
							color: (ctx: { index?: number; tick?: { index?: number } }) => {
								const tickIndex = ctx.index ?? ctx.tick?.index ?? 0;
								const safeIndex = Number.isFinite(Number(tickIndex)) ? Number(tickIndex) : 0;
								return shouldHighlightTick(
									getLabelPSTFromLabels(
										(chart?.data.labels as (string | number | null | undefined)[] | undefined) ??
											[],
										safeIndex
									),
									groupBy,
									safeIndex
								)
									? gridColor
									: gridHighlightColor;
							}
						}
					},
					y: {
						afterFit(axis: { width: number }) {
							axis.width = Y_AXIS_WIDTH;
						},
						ticks: {
							color: textColor
						},
						grid: {
							color: gridColor
						}
					}
				},
				plugins: {
					tooltip: {
						mode: 'index',
						intersect: false,
						backgroundColor: tooltipBackgroundColor,
						titleColor: tooltipTextColor,
						bodyColor: tooltipTextColor,
						footerColor: tooltipTextColor,
						borderColor: tooltipBorderColor,
						borderWidth: 1
					},
					legend: {
						labels: { color: textColor }
					},
					verticalCrosshair: {
						enabled: true,
						line: {
							color: 'rgba(100, 100, 100, 0.8)',
							width: 1,
							dash: [3, 3]
						},
						tooltip: {
							enabled: true,
							delay: 500,
							backgroundColor: tooltipBackgroundColor,
							textColor: tooltipTextColor,
							borderColor: tooltipBorderColor,
							borderWidth: 1,
							borderRadius: 4,
							padding: 8,
							fontSize: 12,
							fontFamily: 'system-ui, sans-serif'
						},
						sync: {
							onHover: (label: string | null) => crosshairStore.setHover(label, CHART_ID),
							getExternalLabel: () => crosshairStore.getExternalLabel(CHART_ID)
						}
					}
				} as Record<string, object>
			}
			// eslint-disable-next-line @typescript-eslint/no-explicit-any
		} as any);

		// Register chart with crosshair store for synchronized crosshairs
		crosshairStore.register(CHART_ID, chart);

		const container = chartCanvas.parentElement;
		if (container) {
			resizeObserver = new ResizeObserver(() => {
				const nextCompactChartMode = isCompactChart();
				if (chart && results.length > 0 && nextCompactChartMode !== compactChartMode) {
					const config = createChartConfig();
					chart.data = config.data;
					// eslint-disable-next-line @typescript-eslint/no-explicit-any
					chart.options = config.options as any;
					compactChartMode = nextCompactChartMode;
					chart.update('none');
					return;
				}
				compactChartMode = nextCompactChartMode;
				chart?.resize();
			});
			resizeObserver.observe(container);
		}

		return () => {
			crosshairStore.unregister(CHART_ID);
			resizeObserver?.disconnect();
			resizeObserver = null;
			chart?.destroy();
			chart = null;
		};
	});

	onDestroy(() => {
		crosshairStore.unregister(CHART_ID);
		if (mirroredRange?.sourceChartId === CHART_ID) {
			rangeSelectionStore.set(null);
		}
		chart?.destroy();
		chart = null;
		resizeObserver?.disconnect();
		resizeObserver = null;
	});

	// Update chart when props change (matches original loadData behavior)
	$effect(() => {
		if (chart && results.length > 0) {
			const config = createChartConfig();
			chart.data = config.data;
			// eslint-disable-next-line @typescript-eslint/no-explicit-any
			chart.options = config.options as any;
			compactChartMode = isCompactChart();
			chart.update('none');
		}
	});

	$effect(() => {
		void theme.dark;
		if (chart) {
			const config = createChartConfig();
			// eslint-disable-next-line @typescript-eslint/no-explicit-any
			chart.options = config.options as any;
			compactChartMode = isCompactChart();
			chart.update('none');
		}
	});
</script>

<div
	class="chart-container"
	role="presentation"
	onmousedown={handleRangeMouseDown}
	onmousemove={handleRangeMouseMove}
	onmouseup={finishRangeSelection}
	onmouseleave={finishRangeSelection}
>
	<canvas bind:this={chartCanvas} class="h-full w-full"></canvas>
	{#if rangeDrag.isDraggingRange && selectionWidth >= MIN_DRAG_PIXELS}
		<div
			class="pointer-events-none absolute border border-gray-500/70 bg-gray-500/20"
			style={`left:${selectionLeft}px; width:${selectionWidth}px; top:${rangeDrag.selectionTop}px; height:${rangeDrag.selectionHeight}px;`}
		></div>
	{/if}
	{#if !rangeDrag.isDraggingRange && mirroredSelectionStyle !== null}
		<div
			class="pointer-events-none absolute border border-gray-500/70 bg-gray-500/20"
			style={mirroredSelectionStyle}
		></div>
	{/if}
</div>

<style>
	.chart-container {
		position: relative;
		height: 100%;
		width: 100%;
	}
</style>
