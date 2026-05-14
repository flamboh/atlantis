<script lang="ts">
	import DragGrip from '$lib/components/common/DragGrip.svelte';
	import { createEventDispatcher, onDestroy, tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { Chart } from './chart-registry';
	import { getRelativePosition } from 'chart.js/helpers';
	import type { ActiveElement, ChartEvent } from 'chart.js';
	import type { GroupByOption, RouterConfig } from '$lib/components/netflow/types.ts';
	import { navigateToNetflowFile } from '$lib/utils/netflow-file-navigation';
	import {
		type IpGranularity,
		type ProtocolMetricKey,
		type ProtocolStatsBucket,
		type ProtocolStatsResponse
	} from '$lib/types/types';
	import {
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
	import { crosshairStore } from '$lib/stores/crosshair';
	import { rangeSelectionStore, type RangeSelectionState } from '$lib/stores/rangeSelection';
	import { theme } from '$lib/stores/theme.svelte';
	import {
		ensureCachedWindow,
		getMissingWindowRanges,
		readCachedWindow,
		type TimeRange
	} from '$lib/utils/window-cache';

	const CHART_ID = 'protocol';

	import {
		dateStringToEpochPST,
		epochToPSTComponents,
		formatDateAsPSTDateString,
		getWeekdayName
	} from '$lib/utils/timezone';

	type SeriesKey = 'uniqueProtocolsIpv4' | 'uniqueProtocolsIpv6';

	const METRIC_LABELS: Record<SeriesKey, string> = {
		uniqueProtocolsIpv4: 'IPv4',
		uniqueProtocolsIpv6: 'IPv6'
	};

	const IP_TO_GROUP_BY: Record<IpGranularity, GroupByOption> = {
		'1d': 'date',
		'1h': 'hour',
		'30m': '30min',
		'5m': '5min'
	};

	const GROUP_BY_TRANSITIONS: Record<GroupByOption, GroupByOption | null> = {
		date: 'hour',
		hour: '30min',
		'30min': '5min',
		'5min': null
	};

	const dispatch = createEventDispatcher<{
		dateChange: { startDate: string; endDate: string };
		groupByChange: { groupBy: GroupByOption };
		metricsChange: { metrics: ProtocolMetricKey[] };
	}>();

	const props = $props<{
		dataset?: string;
		startDate?: string;
		endDate?: string;
		granularity?: IpGranularity;
		routers?: RouterConfig;
		activeMetrics?: ProtocolMetricKey[];
	}>();

	const today = new Date();
	const formatDate = (date: Date): string => formatDateAsPSTDateString(date);
	const getStartDate = () => props.startDate ?? '2025-01-01';
	const getEndDate = () => props.endDate ?? formatDate(today);
	const getGranularity = () => props.granularity ?? '1h';
	const getInitialMetrics = () => props.activeMetrics ?? DEFAULT_METRICS;

	function deriveSelectedRouters(config: RouterConfig | undefined): string[] {
		if (!config) {
			return [];
		}
		return Object.entries(config)
			.filter(([, enabled]) => enabled)
			.map(([name]) => name.trim())
			.filter((name) => name.length > 0)
			.sort();
	}

	const DEFAULT_METRICS: ProtocolMetricKey[] = ['uniqueProtocolsIpv4', 'uniqueProtocolsIpv6'];

	let activeMetrics = $state<ProtocolMetricKey[]>(getInitialMetrics());
	let currentGranularity = $state<IpGranularity>(getGranularity());

	let buckets = $state<ProtocolStatsBucket[]>([]);
	let loading = $state(false);
	let error = $state<string | null>(null);

	let chartCanvas = $state<HTMLCanvasElement | null>(null);
	let chart: Chart<'line'> | null = null;
	let rangeDrag = $state(createRangeDragState());
	let selectionLeft = $derived(Math.min(rangeDrag.dragStartX, rangeDrag.dragCurrentX));
	let selectionWidth = $derived(Math.abs(rangeDrag.dragStartX - rangeDrag.dragCurrentX));
	let mirroredRange = $state<RangeSelectionState | null>(null);

	$effect(() => {
		const unsubscribe = rangeSelectionStore.subscribe((value) => {
			mirroredRange = value;
		});
		return unsubscribe;
	});

	function toEpochSeconds(dateString: string, isEnd = false): number {
		return dateStringToEpochPST(dateString, isEnd);
	}

	function formatBucketLabel(bucket: ProtocolStatsBucket, granularity: IpGranularity): string {
		const pst = epochToPSTComponents(bucket.bucketStart);
		const year = pst.year;
		const month = `${pst.month}`.padStart(2, '0');
		const day = `${pst.day}`.padStart(2, '0');
		const hours = `${pst.hours}`.padStart(2, '0');
		const minutes = `${pst.minutes}`.padStart(2, '0');
		return granularity === '1d'
			? `${year}-${month}-${day}`
			: `${year}-${month}-${day} ${hours}:${minutes}`;
	}

	function formatTickLabel(
		bucketStart: number,
		granularity: IpGranularity,
		_index: number
	): string {
		const pst = epochToPSTComponents(bucketStart);
		const day = pst.day.toString().padStart(2, '0');
		const month = pst.month.toString().padStart(2, '0');
		const hours = pst.hours;
		const minutes = pst.minutes;
		const weekday = getWeekdayName(pst.dayOfWeek);

		if (granularity === '1d') {
			return pst.dayOfWeek === 1 ? `Mon ${month}/${day}` : '';
		}

		if (granularity === '1h') {
			if (hours === 0) {
				return `${weekday} ${pst.month}/${pst.day}`;
			}
			return '';
		}

		if (granularity === '30m') {
			if (minutes === 0 && (hours === 0 || hours === 12)) {
				return `${weekday} ${pst.month}/${pst.day} ${hours.toString().padStart(2, '0')}:00`;
			}
			return '';
		}

		if (granularity === '5m') {
			if (minutes === 0) {
				return `${weekday} ${pst.month}/${pst.day} ${hours.toString().padStart(2, '0')}:00`;
			}
			return '';
		}

		return '';
	}

	function shouldHighlightTick(
		bucketStart: number,
		granularity: IpGranularity,
		index: number
	): boolean {
		const pst = epochToPSTComponents(bucketStart);
		const hours = pst.hours;
		const minutes = pst.minutes;

		if (granularity === '1d') {
			return pst.dayOfWeek === 1;
		}
		if (granularity === '1h') {
			return hours === 0;
		}
		if (granularity === '30m') {
			return minutes === 0 && (hours === 0 || hours === 12);
		}
		if (granularity === '5m') {
			return minutes === 0;
		}
		return index === 0;
	}

	const HUE_STEP = 110;
	const SERIES_COLOR: Record<SeriesKey, { hue: number; saturation: number; lightness: number }> = {
		uniqueProtocolsIpv4: { hue: 210, saturation: 70, lightness: 50 },
		uniqueProtocolsIpv6: { hue: 35, saturation: 75, lightness: 48 }
	};

	function buildColors(key: SeriesKey, routerIndex: number) {
		const base = SERIES_COLOR[key];
		const hue = (base.hue + routerIndex * HUE_STEP) % 360;
		const stroke = `hsl(${hue}, ${base.saturation}%, ${base.lightness}%)`;
		const fill = `hsla(${hue}, ${base.saturation}%, ${base.lightness}%, 0.2)`;
		return { stroke, fill };
	}

	function destroyChart() {
		if (chart) {
			crosshairStore.unregister(CHART_ID);
			chart.destroy();
			chart = null;
		}
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

	function applyChartTheme() {
		if (!chart) {
			return;
		}

		const {
			textColor,
			gridColor,
			gridHighlightColor,
			tooltipBackgroundColor,
			tooltipTextColor,
			tooltipBorderColor
		} = getChartColors();
		const scales = chart.options.scales;

		if (scales?.x) {
			scales.x.title = { ...scales.x.title, color: textColor };
			scales.x.ticks = { ...scales.x.ticks, color: textColor };
			scales.x.grid = { ...scales.x.grid, color: scales.x.grid?.color ?? gridHighlightColor };
		}

		if (scales?.y) {
			scales.y.title = { ...scales.y.title, color: textColor };
			scales.y.ticks = { ...scales.y.ticks, color: textColor };
			scales.y.grid = { ...scales.y.grid, color: gridColor };
		}

		chart.options.plugins = {
			...chart.options.plugins,
			legend: { position: 'top', labels: { color: textColor } },
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
		} as Record<string, unknown>;

		chart.update('none');
	}

	function emitDrilldown(nextGroupBy: GroupByOption, start: Date, end: Date) {
		dispatch('groupByChange', { groupBy: nextGroupBy });
		dispatch('dateChange', {
			startDate: formatDate(start),
			endDate: formatDate(end)
		});
	}

	function handleMetricToggle(metric: ProtocolMetricKey) {
		const nextMetrics = activeMetrics.includes(metric)
			? activeMetrics.filter((item) => item !== metric)
			: [...activeMetrics, metric];
		activeMetrics = nextMetrics;
		dispatch('metricsChange', { metrics: nextMetrics });
	}

	function publishRangeSelection(startIndex: number, endIndex: number) {
		const labels = getSelectionLabels(chart, startIndex, endIndex);
		if (!labels) return;
		rangeSelectionStore.set({ sourceChartId: CHART_ID, ...labels });
	}

	function applyRangeDrilldown(startIndex: number, endIndex: number) {
		if (!chart?.data.labels) return;
		const labels = chart.data.labels as string[];
		const from = Math.max(0, Math.min(labels.length - 1, Math.min(startIndex, endIndex)));
		const to = Math.max(0, Math.min(labels.length - 1, Math.max(startIndex, endIndex)));
		const startLabel = labels[from];
		const endLabel = labels[to];
		if (!startLabel || !endLabel) return;

		const groupBy = IP_TO_GROUP_BY[currentGranularity];
		if (!groupBy) return;

		const startDate = parseClickedLabel(startLabel, groupBy);
		const endBucketStart = parseClickedLabel(endLabel, groupBy);
		if (Number.isNaN(startDate.getTime()) || Number.isNaN(endBucketStart.getTime())) return;
		const endExclusive = new Date(endBucketStart.getTime() + groupByBucketDurationMs(groupBy));
		const selectedRangeMs = endExclusive.getTime() - startDate.getTime();
		const nextGroupBy = chooseAdaptiveGranularity(selectedRangeMs);
		emitDrilldown(nextGroupBy, startDate, endExclusive);
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

	function getLabelFromIndex(index: number): string | null {
		if (!chart || !chart.data.labels) return null;
		const labels = chart.data.labels as string[];
		if (index < 0 || index >= labels.length) return null;
		return labels[index] ?? null;
	}

	function handleChartClick(event: ChartEvent, activeElements: ActiveElement[]) {
		if (rangeDrag.suppressNextClick) {
			rangeDrag.suppressNextClick = false;
			return;
		}
		if (!chart || !chart.data.labels) return;

		const groupBy = IP_TO_GROUP_BY[currentGranularity];
		if (!groupBy) return;

		const labels = chart.data.labels as string[];
		if (labels.length === 0) return;

		const canvasPosition = getRelativePosition(event, chart);
		const dataX = chart.scales.x.getValueForPixel(canvasPosition.x);

		let labelIndex: number | null = null;
		if (typeof dataX === 'number' && Number.isFinite(dataX)) {
			const roundedIndex = Math.round(dataX);
			if (roundedIndex >= 0 && roundedIndex < labels.length) {
				labelIndex = roundedIndex;
			} else if (roundedIndex < 0) {
				labelIndex = 0;
			} else {
				labelIndex = labels.length - 1;
			}
		}

		const fallbackIndex = activeElements.length > 0 ? activeElements[0].index : null;
		const targetIndex = labelIndex ?? fallbackIndex;
		const label = targetIndex !== null ? getLabelFromIndex(targetIndex) : labels[0];

		if (!label) return;

		const clickedDate = parseClickedLabel(label, groupBy);
		if (!(clickedDate instanceof Date) || Number.isNaN(clickedDate.getTime())) return;
		const activeLabel = fallbackIndex !== null ? getLabelFromIndex(fallbackIndex) : null;

		if (groupBy === '5min') {
			const labelForSlug = activeLabel ?? label;
			const slug = generateSlugFromLabel(labelForSlug, '5min');
			if (slug) {
				void navigateToNetflowFile(goto, slug, props.dataset);
			}
			return;
		}

		const nextGroupBy = GROUP_BY_TRANSITIONS[groupBy];
		if (!nextGroupBy) return;

		if (groupBy === 'date') {
			const rangeStart = new Date(clickedDate.getTime() - 15 * 24 * 60 * 60 * 1000);
			const rangeEnd = new Date(clickedDate.getTime() + 16 * 24 * 60 * 60 * 1000);
			emitDrilldown(nextGroupBy, rangeStart, rangeEnd);
		} else if (groupBy === 'hour') {
			const rangeStart = new Date(clickedDate.getTime() - 3 * 24 * 60 * 60 * 1000);
			const rangeEnd = new Date(clickedDate.getTime() + 4 * 24 * 60 * 60 * 1000);
			emitDrilldown(nextGroupBy, rangeStart, rangeEnd);
		} else if (groupBy === '30min') {
			const rangeEnd = new Date(clickedDate.getTime() + 24 * 60 * 60 * 1000);
			emitDrilldown(nextGroupBy, clickedDate, rangeEnd);
		}
	}

	function renderChart() {
		const {
			textColor,
			gridColor,
			gridHighlightColor,
			tooltipBackgroundColor,
			tooltipTextColor,
			tooltipBorderColor
		} = getChartColors();
		const selectedRouters = new Set(deriveSelectedRouters(props.routers));
		const selectedBuckets = buckets.filter((bucket) => selectedRouters.has(bucket.router));

		if (activeMetrics.length === 0 || selectedBuckets.length === 0) {
			destroyChart();
			return;
		}

		const canvas = chartCanvas;
		if (!canvas) return;

		const bucketStarts = Array.from(
			new Set(selectedBuckets.map((bucket) => bucket.bucketStart))
		).sort((a, b) => a - b);
		const routers = Array.from(new Set(selectedBuckets.map((bucket) => bucket.router))).sort();

		const labelSamples = new Map<number, ProtocolStatsBucket>();
		selectedBuckets.forEach((bucket) => {
			if (!labelSamples.has(bucket.bucketStart)) {
				labelSamples.set(bucket.bucketStart, bucket);
			}
		});

		const labels = bucketStarts.map((bucketStart) => {
			const bucket = labelSamples.get(bucketStart);
			return bucket ? formatBucketLabel(bucket, currentGranularity) : '';
		});

		const bucketMap = new Map<string, ProtocolStatsBucket>();
		selectedBuckets.forEach((bucket) => {
			bucketMap.set(`${bucket.router}-${bucket.bucketStart}`, bucket);
		});

		const datasets = routers.flatMap((router, routerIndex) =>
			(['uniqueProtocolsIpv4', 'uniqueProtocolsIpv6'] as const)
				.filter((key) => activeMetrics.includes(key))
				.map((key) => {
					const { stroke, fill } = buildColors(key, routerIndex);
					const data = bucketStarts.map((bucketStart) => {
						const bucket = bucketMap.get(`${router}-${bucketStart}`);
						return bucket ? bucket[key] : null;
					});

					return {
						label: `${router} · ${METRIC_LABELS[key]}`,
						data,
						borderColor: stroke,
						backgroundColor: fill,
						tension: 0.3,
						fill: false,
						pointRadius: 0,
						pointHoverRadius: 4,
						spanGaps: true
					};
				})
		);

		if (datasets.length === 0 || labels.length === 0) {
			if (chart) {
				chart.data.labels = [];
				chart.data.datasets = [];
				chart.update();
			}
			return;
		}

		if (!chart) {
			chart = new Chart(canvas, {
				type: 'line',
				data: { labels, datasets },
				options: {
					onClick: handleChartClick,
					responsive: true,
					maintainAspectRatio: false,
					animation: false,
					interaction: { mode: 'index', intersect: false },
					plugins: {
						legend: { position: 'top', labels: { color: textColor } },
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
					} as Record<string, unknown>,
					scales: {
						x: {
							title: { display: true, text: `Time (${currentGranularity})`, color: textColor },
							ticks: {
								color: textColor,
								autoSkip: false,
								maxRotation: 45,
								minRotation: 45,
								sampleSize: 12,
								callback: (_value, idx) =>
									formatTickLabel(
										bucketStarts[idx as number] ?? 0,
										currentGranularity,
										idx as number
									)
							},
							grid: {
								color: (ctx) =>
									shouldHighlightTick(bucketStarts[ctx.index] ?? 0, currentGranularity, ctx.index)
										? gridColor
										: gridHighlightColor
							}
						},
						y: {
							beginAtZero: true,
							afterFit(axis: { width: number }) {
								axis.width = Y_AXIS_WIDTH;
							},
							title: { display: true, text: 'Unique Protocols', color: textColor },
							ticks: { color: textColor },
							grid: { color: gridColor }
						}
					}
				}
			});
			// Register chart with crosshair store for synchronized crosshairs
			crosshairStore.register(CHART_ID, chart);
		} else {
			chart.data.labels = labels;
			chart.data.datasets = datasets as never[];
			chart.options.scales = {
				x: {
					...chart.options.scales?.x,
					title: { display: true, text: `Time (${currentGranularity})`, color: textColor },
					ticks: {
						color: textColor,
						autoSkip: false,
						maxRotation: 45,
						minRotation: 45,
						sampleSize: 12,
						callback: (_value, idx) =>
							formatTickLabel(bucketStarts[idx as number] ?? 0, currentGranularity, idx as number)
					},
					grid: {
						color: (ctx) =>
							shouldHighlightTick(bucketStarts[ctx.index] ?? 0, currentGranularity, ctx.index)
								? gridColor
								: gridHighlightColor
					}
				},
				y: {
					...chart.options.scales?.y,
					afterFit(axis: { width: number }) {
						axis.width = Y_AXIS_WIDTH;
					},
					title: { display: true, text: 'Unique Protocols', color: textColor },
					ticks: { color: textColor },
					grid: { color: gridColor }
				}
			};
			chart.options.onClick = handleChartClick;
			chart.options.plugins = {
				...chart.options.plugins,
				legend: { position: 'top', labels: { color: textColor } },
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
			} as Record<string, unknown>;
			chart.update('none');
		}
	}

	type FilterInputs = {
		startDate: string;
		endDate: string;
		granularity: IpGranularity;
		routers: string[];
	};

	let lastFiltersKey = '';
	let lastIncomingMetricsKey = '';
	let requestToken = 0;

	function getRequestedRange(filters: FilterInputs): TimeRange {
		return {
			start: toEpochSeconds(filters.startDate),
			end: toEpochSeconds(filters.endDate, true)
		};
	}

	function getCacheKey(filters: FilterInputs): string {
		return JSON.stringify({
			chart: CHART_ID,
			dataset: props.dataset ?? '',
			granularity: filters.granularity,
			routers: filters.routers
		});
	}

	async function loadData(filters: FilterInputs, token: number) {
		const requestedRange = getRequestedRange(filters);
		const cacheKey = getCacheKey(filters);
		loading = getMissingWindowRanges(cacheKey, requestedRange).length > 0;
		error = null;
		if (loading) {
			destroyChart();
		}

		const params = new URLSearchParams({
			dataset: props.dataset ?? '',
			granularity: filters.granularity,
			routers: filters.routers.join(',')
		});

		try {
			await ensureCachedWindow<ProtocolStatsBucket>({
				key: cacheKey,
				requestedRange,
				fetchRange: async (range) => {
					const response = await fetch(
						`/api/protocol/stats?${new URLSearchParams({
							...Object.fromEntries(params.entries()),
							startDate: range.start.toString(),
							endDate: range.end.toString()
						}).toString()}`
					);
					if (!response.ok) {
						const message = await response.text();
						throw new Error(message || 'Failed to load protocol statistics');
					}
					const data = (await response.json()) as ProtocolStatsResponse;
					return data.buckets;
				},
				getRecordKey: (record) => `${record.router}-${record.bucketStart}`,
				compareRecords: (left, right) =>
					left.bucketStart - right.bucketStart || left.router.localeCompare(right.router)
			});
			if (token !== requestToken) return;
			buckets = readCachedWindow<ProtocolStatsBucket>(cacheKey, requestedRange, (record, range) => {
				return record.bucketStart >= range.start && record.bucketStart < range.end;
			});
			loading = false;
			await tick();
			renderChart();
		} catch (err) {
			if (token !== requestToken) return;
			error = err instanceof Error ? err.message : 'Unexpected error loading protocol statistics';
			buckets = [];
			loading = false;
			destroyChart();
		}
	}

	onDestroy(() => {
		if (mirroredRange?.sourceChartId === CHART_ID) {
			rangeSelectionStore.set(null);
		}
		destroyChart();
	});

	$effect(() => {
		void theme.dark;
		if (chart) {
			applyChartTheme();
		}
	});

	$effect(() => {
		const incomingMetrics = props.activeMetrics ?? DEFAULT_METRICS;
		const nextKey = JSON.stringify(incomingMetrics);
		if (nextKey === lastIncomingMetricsKey) {
			return;
		}
		lastIncomingMetricsKey = nextKey;
		activeMetrics = [...incomingMetrics];
		void (async () => {
			await tick();
			renderChart();
		})();
	});

	$effect(() => {
		const routerConfig = props.routers;
		if (!routerConfig || Object.keys(routerConfig).length === 0) {
			return;
		}

		const selectedRouters = deriveSelectedRouters(routerConfig);

		const filters: FilterInputs = {
			startDate: getStartDate(),
			endDate: getEndDate(),
			granularity: getGranularity(),
			routers: selectedRouters
		};

		currentGranularity = filters.granularity;

		if (selectedRouters.length === 0) {
			error = 'Select at least one source to view protocol statistics';
			buckets = [];
			destroyChart();
			lastFiltersKey = JSON.stringify({ ...filters, selectedRouters });
			loading = false;
			return;
		}

		error = null;
		const nextKey = JSON.stringify({ ...filters, selectedRouters });
		if (nextKey === lastFiltersKey) return;

		lastFiltersKey = nextKey;
		const token = ++requestToken;
		loadData(filters, token);
	});
</script>

<div class="dark:border-dark-border dark:bg-dark-surface rounded-lg border bg-white shadow-sm">
	<div
		class="dark:border-dark-border relative cursor-grab border-b p-4 select-none active:cursor-grabbing"
		draggable="true"
		data-drag-handle
	>
		<h3 class="text-lg font-semibold text-gray-900 dark:text-gray-100">Unique Protocol Counts</h3>
		<DragGrip />
	</div>
	<div class="p-4">
		<div class="mb-4 flex flex-wrap items-center gap-4">
			{#each ['uniqueProtocolsIpv4', 'uniqueProtocolsIpv6'] as const as key (key)}
				<label class="flex cursor-pointer items-center gap-2">
					<input
						type="checkbox"
						checked={activeMetrics.includes(key)}
						onchange={() => handleMetricToggle(key)}
						class="h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
					/>
					<span class="text-sm text-gray-700 dark:text-gray-300">
						{key === 'uniqueProtocolsIpv4' ? 'Unique Protocols IPv4' : 'Unique Protocols IPv6'}
					</span>
				</label>
			{/each}
		</div>

		<div
			class="dark:border-dark-border dark:bg-dark-subtle/60 relative h-[320px] min-h-[240px] resize-y overflow-auto rounded-md border border-gray-200 bg-white/60"
			role="presentation"
			onmousedown={handleRangeMouseDown}
			onmousemove={handleRangeMouseMove}
			onmouseup={finishRangeSelection}
			onmouseleave={finishRangeSelection}
		>
			{#if loading}
				<div class="flex h-full items-center justify-center">
					<div class="text-gray-500 dark:text-gray-400">Loading protocol data...</div>
				</div>
			{:else if error}
				<div class="flex h-full items-center justify-center">
					<div class="text-red-500">{error}</div>
				</div>
			{:else if activeMetrics.length === 0}
				<div class="flex h-full items-center justify-center">
					<div class="text-gray-500 dark:text-gray-400">Select at least one metric to display.</div>
				</div>
			{:else if buckets.length === 0}
				<div class="flex h-full items-center justify-center">
					<div class="text-gray-500 dark:text-gray-400">
						No protocol data for the selected window.
					</div>
				</div>
			{:else}
				<div class="h-full">
					<canvas bind:this={chartCanvas} aria-label="Protocol chart"></canvas>
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
			{/if}
		</div>
	</div>
</div>
