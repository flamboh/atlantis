import type { Chart } from 'chart.js';
import type { GroupByOption, NetflowDataPoint } from '$lib/components/netflow/types.ts';
import type { RangeSelectionState } from '$lib/stores/rangeSelection';
import {
	parseLabelToPSTComponents,
	parseLabelToDateForDrilldown,
	epochToPSTComponents,
	getWeekdayName,
	type PSTDateComponents
} from '$lib/utils/timezone';

/** Fixed y-axis width (px) for consistent chart alignment */
export const Y_AXIS_WIDTH = 80;
export const MIN_DRAG_PIXELS = 6;

export interface RangeDragState {
	isDraggingRange: boolean;
	dragStartX: number;
	dragCurrentX: number;
	selectionTop: number;
	selectionHeight: number;
	suppressNextClick: boolean;
}

export function createRangeDragState(): RangeDragState {
	return {
		isDraggingRange: false,
		dragStartX: 0,
		dragCurrentX: 0,
		selectionTop: 0,
		selectionHeight: 0,
		suppressNextClick: false
	};
}

export function getChartArea(chart: Chart | null) {
	if (!chart) return null;
	return chart.chartArea;
}

export function clampToChartX(chart: Chart | null, x: number): number {
	const area = getChartArea(chart);
	if (!area) return x;
	return Math.max(area.left, Math.min(area.right, x));
}

export function indexFromPixelX(chart: Chart | null, pixelX: number): number | null {
	if (!chart?.data.labels) return null;
	const labels = chart.data.labels;
	if (!labels || labels.length === 0) return null;
	const value = chart.scales.x.getValueForPixel(pixelX);
	if (typeof value !== 'number' || Number.isNaN(value)) return null;
	if ((chart.scales.x.options as { type?: string }).type === 'linear') {
		const points = chart.data.datasets[0]?.data ?? [];
		let closestIndex: number | null = null;
		let closestDistance = Infinity;
		const seenX = new Set<number>();
		points.forEach((point, index) => {
			if (typeof point !== 'object' || point === null || Array.isArray(point) || !('x' in point))
				return;
			const x = point.x;
			if (typeof x !== 'number' || !Number.isFinite(x)) return;
			if (seenX.has(x)) return;
			seenX.add(x);
			const distance = Math.abs(x - value);
			if (distance < closestDistance) {
				closestDistance = distance;
				closestIndex = index;
			}
		});
		return closestIndex;
	}
	const rounded = Math.round(value);
	return Math.max(0, Math.min(labels.length - 1, rounded));
}

export function getSelectionLabels(
	chart: Chart | null,
	startIndex: number,
	endIndex: number
): { startLabel: string; endLabel: string } | null {
	if (!chart?.data.labels) return null;
	const labels = chart.data.labels as string[];
	if (labels.length === 0) return null;
	const from = Math.max(0, Math.min(labels.length - 1, Math.min(startIndex, endIndex)));
	const to = Math.max(0, Math.min(labels.length - 1, Math.max(startIndex, endIndex)));
	const startLabel = labels[from];
	const endLabel = labels[to];
	if (!startLabel || !endLabel) return null;
	return { startLabel, endLabel };
}

export function beginRangeDrag(
	state: RangeDragState,
	event: MouseEvent,
	chartCanvas: HTMLCanvasElement | null,
	chart: Chart | null,
	onPreviewRange?: (startIndex: number, endIndex: number) => void
): void {
	if (event.button !== 0 || !chartCanvas) return;
	const rect = chartCanvas.getBoundingClientRect();
	const x = event.clientX - rect.left;
	const y = event.clientY - rect.top;
	const area = getChartArea(chart);
	if (!area) return;
	if (x < area.left || x > area.right || y < area.top || y > area.bottom) return;
	state.isDraggingRange = true;
	state.dragStartX = clampToChartX(chart, x);
	state.dragCurrentX = state.dragStartX;
	state.selectionTop = area.top;
	state.selectionHeight = area.bottom - area.top;
	const startIndex = indexFromPixelX(chart, state.dragStartX);
	if (startIndex !== null) {
		onPreviewRange?.(startIndex, startIndex);
	}
}

export function updateRangeDrag(
	state: RangeDragState,
	event: MouseEvent,
	chartCanvas: HTMLCanvasElement | null,
	chart: Chart | null,
	onPreviewRange?: (startIndex: number, endIndex: number) => void
): void {
	if (!state.isDraggingRange || !chartCanvas) return;
	const rect = chartCanvas.getBoundingClientRect();
	const x = event.clientX - rect.left;
	state.dragCurrentX = clampToChartX(chart, x);
	const startIndex = indexFromPixelX(chart, state.dragStartX);
	const endIndex = indexFromPixelX(chart, state.dragCurrentX);
	if (startIndex !== null && endIndex !== null) {
		onPreviewRange?.(startIndex, endIndex);
	}
}

export function endRangeDrag(
	state: RangeDragState,
	chart: Chart | null,
	onCommitRange: (startIndex: number, endIndex: number) => void
): void {
	if (!state.isDraggingRange) return;
	const wasDrag = Math.abs(state.dragCurrentX - state.dragStartX) >= MIN_DRAG_PIXELS;
	if (wasDrag) {
		const startIndex = indexFromPixelX(chart, state.dragStartX);
		const endIndex = indexFromPixelX(chart, state.dragCurrentX);
		if (startIndex !== null && endIndex !== null) {
			onCommitRange(startIndex, endIndex);
			state.suppressNextClick = true;
		}
	}
	state.isDraggingRange = false;
}

export function buildMirroredSelectionStyle(
	chart: Chart | null,
	mirroredRange: RangeSelectionState | null,
	sourceChartId: string
): string | null {
	if (!mirroredRange || mirroredRange.sourceChartId === sourceChartId || !chart?.data.labels)
		return null;
	const labels = chart.data.labels as string[];
	const startIndex = labels.indexOf(mirroredRange.startLabel);
	const endIndex = labels.indexOf(mirroredRange.endLabel);
	if (startIndex === -1 || endIndex === -1) return null;
	const area = getChartArea(chart);
	if (!area) return null;
	const from = Math.min(startIndex, endIndex);
	const to = Math.max(startIndex, endIndex);
	const firstDataset = chart.data.datasets[0]?.data ?? [];
	const xValues = firstDataset.reduce<number[]>((values, point) => {
		if (
			typeof point === 'object' &&
			point !== null &&
			!Array.isArray(point) &&
			'x' in point &&
			typeof point.x === 'number' &&
			Number.isFinite(point.x) &&
			!values.includes(point.x)
		) {
			values.push(point.x);
		}
		return values;
	}, []);
	const xValueAt = (index: number): number => {
		if ((chart.scales.x.options as { type?: string }).type === 'linear') {
			const uniqueX = xValues[index];
			if (uniqueX !== undefined) return uniqueX;
		}
		const point = firstDataset[index];
		if (
			(chart.scales.x.options as { type?: string }).type === 'linear' &&
			typeof point === 'object' &&
			point !== null &&
			!Array.isArray(point) &&
			'x' in point
		) {
			const x = point.x;
			if (typeof x === 'number' && Number.isFinite(x)) return x;
		}
		return index;
	};
	const rawLeft = chart.scales.x.getPixelForValue(xValueAt(from));
	const rawRight = chart.scales.x.getPixelForValue(xValueAt(to));
	const left = Math.max(area.left, Math.min(rawLeft, rawRight));
	const right = Math.min(area.right, Math.max(rawLeft, rawRight));
	const width = right - left;
	if (!Number.isFinite(width) || width < MIN_DRAG_PIXELS) return null;
	return `left:${left}px; width:${width}px; top:${area.top}px; height:${area.bottom - area.top}px;`;
}

/**
 * Format labels from NetFlow data points using PST timezone.
 * item.bucketStart is epoch seconds from the API.
 */
export function formatLabels(results: NetflowDataPoint[], groupBy: GroupByOption): string[] {
	return results.map((item) => {
		const epoch = item.bucketStart;
		if (!Number.isFinite(epoch)) {
			return '';
		}

		// Convert to PST components for consistent display
		const pst = epochToPSTComponents(epoch);
		const year = pst.year;
		const month = String(pst.month).padStart(2, '0');
		const day = String(pst.day).padStart(2, '0');
		const hours = String(pst.hours).padStart(2, '0');
		const minutes = String(pst.minutes).padStart(2, '0');

		switch (groupBy) {
			case 'date':
				return `${year}-${month}-${day}`;
			case 'hour':
				return `${year}-${month}-${day} ${hours}:00`;
			case '30min':
			case '5min':
				return `${year}-${month}-${day} ${hours}:${minutes}`;
			default:
				return `${year}-${month}-${day}`;
		}
	});
}

export function getXAxisTitle(groupBy: GroupByOption): string {
	switch (groupBy) {
		case 'date':
			return 'Date';
		case 'hour':
			return 'Hour';
		case '30min':
			return '30 Minutes';
		case '5min':
			return '5 Minutes';
		default:
			return 'Time';
	}
}

export function formatNumber(value: number): string {
	if (value >= 1e15) return (value / 1e15).toFixed(1) + 'P';
	if (value >= 1e12) return (value / 1e12).toFixed(1) + 'T';
	if (value >= 1e9) return (value / 1e9).toFixed(1) + 'B';
	if (value >= 1e6) return (value / 1e6).toFixed(1) + 'M';
	if (value >= 1e3) return (value / 1e3).toFixed(1) + 'K';
	return value.toString();
}

export function generateColors(count: number): string[] {
	const colors = [
		'#FF6384',
		'#36A2EB',
		'#FFCE56',
		'#4BC0C0',
		'#9966FF',
		'#FF9F40',
		'#FF6384',
		'#C9CBCF',
		'#4BC0C0',
		'#FF6384',
		'#36A2EB',
		'#FFCE56',
		'#4BC0C0',
		'#9966FF',
		'#FF9F40',
		'#FF6384',
		'#C9CBCF',
		'#4BC0C0',
		'#FF6384',
		'#36A2EB'
	];
	return colors.slice(0, count);
}

/**
 * Parse a clicked chart label into a Date for drill-down calculations.
 * Returns a Date that represents the PST moment (for time arithmetic).
 */
export function parseClickedLabel(label: string, _groupBy: GroupByOption): Date {
	const date = parseLabelToDateForDrilldown(label);
	return date ?? new Date(NaN);
}

export function generateSlugFromLabel(label: string, groupBy: GroupByOption): string {
	if (groupBy === '5min') {
		const [datePart, timePart] = label.split(' ');
		const [year, month, day] = datePart.split('-');
		const [hour, minute] = timePart.split(':');
		return `${year}${month}${day}${hour}${minute}`;
	}
	return '';
}

export function groupByBucketDurationMs(groupBy: GroupByOption): number {
	if (groupBy === 'date') return 24 * 60 * 60 * 1000;
	if (groupBy === 'hour') return 60 * 60 * 1000;
	if (groupBy === '30min') return 30 * 60 * 1000;
	return 5 * 60 * 1000;
}

const GROUP_BY_DETAIL_LEVEL: Record<GroupByOption, number> = {
	date: 0,
	hour: 1,
	'30min': 2,
	'5min': 3
};

function parseDateInputToUtcMs(value: string): number | null {
	const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
	if (!match) {
		return null;
	}

	const [, year, month, day] = match;
	return Date.UTC(Number(year), Number(month) - 1, Number(day));
}

export function getInclusiveDateRangeMs(startDate: string, endDate: string): number | null {
	const oneDay = 24 * 60 * 60 * 1000;
	const startMs = parseDateInputToUtcMs(startDate);
	const endMs = parseDateInputToUtcMs(endDate);

	if (startMs === null || endMs === null || endMs < startMs) {
		return null;
	}

	return endMs - startMs + oneDay;
}

/**
 * Choose granularity from selected range duration.
 * Thresholds are calibrated from existing click drilldown windows:
 * 1 day -> 5min, 7 days -> 30min, 31 days -> hour.
 */
export function chooseAdaptiveGranularity(rangeMs: number): GroupByOption {
	const oneDay = 24 * 60 * 60 * 1000;
	const sevenDays = 7 * oneDay;
	const thirtyOneDays = 31 * oneDay;

	// Midpoints between known-good drilldown windows.
	const fiveMinCutoff = (oneDay + sevenDays) / 2; // 4 days
	const thirtyMinCutoff = (sevenDays + thirtyOneDays) / 2; // 19 days
	const hourCutoff = thirtyOneDays * 2; // 62 days

	if (rangeMs <= fiveMinCutoff) return '5min';
	if (rangeMs <= thirtyMinCutoff) return '30min';
	if (rangeMs <= hourCutoff) return 'hour';
	return 'date';
}

export function getMaxAllowedGranularityForDateRange(
	startDate: string,
	endDate: string
): GroupByOption | null {
	const rangeMs = getInclusiveDateRangeMs(startDate, endDate);
	if (rangeMs === null) {
		return null;
	}

	return chooseAdaptiveGranularity(rangeMs);
}

export function isGranularityAllowedForDateRange(
	groupBy: GroupByOption,
	startDate: string,
	endDate: string
): boolean {
	const maxAllowed = getMaxAllowedGranularityForDateRange(startDate, endDate);
	if (maxAllowed === null) {
		return true;
	}

	return GROUP_BY_DETAIL_LEVEL[groupBy] <= GROUP_BY_DETAIL_LEVEL[maxAllowed];
}

export function clampGroupByToDateRange(
	groupBy: GroupByOption,
	startDate: string,
	endDate: string
): GroupByOption {
	const maxAllowed = getMaxAllowedGranularityForDateRange(startDate, endDate);
	if (maxAllowed === null || isGranularityAllowedForDateRange(groupBy, startDate, endDate)) {
		return groupBy;
	}

	return maxAllowed;
}

/**
 * Parse a label to PST components for tick formatting.
 * Uses timezone-aware parsing to ensure consistent display regardless of viewer's timezone.
 */
function safePSTComponents(label: string): PSTDateComponents | null {
	return parseLabelToPSTComponents(label);
}

export function formatNetflowTick(
	groupBy: GroupByOption,
	label: string | undefined,
	index: number
): string {
	if (!label) return '';
	const pst = safePSTComponents(label);
	if (!pst) return '';

	const weekday = getWeekdayName(pst.dayOfWeek);
	const month = pst.month;
	const day = pst.day;
	const hours = pst.hours;
	const minutes = pst.minutes;

	if (groupBy === 'date') {
		return pst.dayOfWeek === 1 ? `${weekday} ${month}/${day}` : '';
	}

	if (groupBy === 'hour') {
		return hours === 0 ? `${weekday} ${month}/${day}` : '';
	}

	if (groupBy === '30min') {
		if (minutes === 0 && (hours === 0 || hours === 12)) {
			return `${weekday} ${month}/${day} ${hours.toString().padStart(2, '0')}:00`;
		}
		return '';
	}

	if (groupBy === '5min') {
		if (minutes === 0) {
			return `${weekday} ${month}/${day} ${hours.toString().padStart(2, '0')}:00`;
		}
		return '';
	}

	return index === 0 ? `${weekday} ${month}/${day}` : '';
}

export function shouldHighlightNetflowGrid(
	groupBy: GroupByOption,
	label: string | undefined,
	index: number
): boolean {
	if (!label) return index === 0;
	const pst = safePSTComponents(label);
	if (!pst) return index === 0;

	const hours = pst.hours;
	const minutes = pst.minutes;

	if (groupBy === 'date') {
		return pst.dayOfWeek === 1;
	}
	if (groupBy === 'hour') {
		return hours === 0;
	}
	if (groupBy === '30min') {
		return minutes === 0 && (hours === 0 || hours === 12);
	}
	if (groupBy === '5min') {
		return minutes === 0;
	}
	return index === 0;
}

/**
 * Coverage metadata attached to a time bucket. Keep this structural so chart
 * presentation code can consume API buckets without coupling to a database
 * response type.
 */
export type ChartCoverage = {
	state: 'complete' | 'partial' | 'unknown';
	observedUnits: number;
	expectedUnits: number;
};

export type ChartTimeBucket<T> = {
	bucketStart: number;
	bucketEnd: number;
	coverage: ChartCoverage;
	data: T | null;
};

export type TemporalChartPoint<T = number> = {
	x: number;
	y: T | null;
	bucketStart: number;
	bucketEnd: number;
	coverage: ChartCoverage;
};

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null;
}

/** Read coverage metadata from an untyped Chart.js point. */
export function getChartBucketCoverage(bucket: unknown): ChartCoverage | null {
	if (!isRecord(bucket) || !isRecord(bucket.coverage)) {
		return null;
	}

	const { state, observedUnits, expectedUnits } = bucket.coverage;
	if (
		(state === 'complete' || state === 'partial' || state === 'unknown') &&
		typeof observedUnits === 'number' &&
		Number.isFinite(observedUnits) &&
		typeof expectedUnits === 'number' &&
		Number.isFinite(expectedUnits)
	) {
		return { state, observedUnits, expectedUnits };
	}

	return null;
}

/** Build Chart.js points without compressing gaps into adjacent indexes. */
export function buildTemporalChartPoints<T>(
	buckets: readonly ChartTimeBucket<T>[],
	getValue: (data: T) => number | null
): TemporalChartPoint[] {
	return buckets.map((bucket) => ({
		x: bucket.bucketStart,
		y: bucket.data === null ? null : getValue(bucket.data),
		bucketStart: bucket.bucketStart,
		bucketEnd: bucket.bucketEnd,
		coverage: bucket.coverage
	}));
}

/** Return the visual treatment used for a complete or partial data point. */
export function getCoveragePointStyle(
	coverage: ChartCoverage,
	color: string
): {
	backgroundColor: string;
	borderColor: string;
	borderWidth: number;
	radius: number;
} {
	if (coverage.state !== 'partial') {
		return {
			backgroundColor: color,
			borderColor: color,
			borderWidth: 1,
			radius: 0
		};
	}

	return {
		backgroundColor: 'transparent',
		borderColor: color,
		borderWidth: 2,
		radius: 4
	};
}

/** Partial buckets make both adjoining line segments visibly uncertain. */
export function isCoverageSegmentDashed(
	left: { coverage?: ChartCoverage } | null | undefined,
	right: { coverage?: ChartCoverage } | null | undefined
): boolean {
	return left?.coverage?.state === 'partial' || right?.coverage?.state === 'partial';
}

export function getCoverageTooltipLines(coverage: ChartCoverage | null | undefined): string[] {
	if (!coverage || coverage.state === 'complete') {
		return [];
	}

	return [
		`Coverage: ${coverage.state}`,
		`Observed units: ${coverage.observedUnits.toLocaleString()} / ${coverage.expectedUnits.toLocaleString()}`
	];
}
