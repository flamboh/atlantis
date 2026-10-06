import type { GroupByOption, NetflowDataPoint } from '#lib/components/netflow/types.ts';
import {
	parseLabelToPSTComponents,
	parseLabelToDateForDrilldown,
	epochToPSTComponents,
	getWeekdayName,
	type PSTDateComponents
} from '#lib/utils/timezone.ts';

/** Fixed y-axis width (px) for consistent chart alignment */
export const Y_AXIS_WIDTH = 80;
export const MIN_DRAG_PIXELS = 6;

/** Return the index of the finite value nearest to a target. */
export function findNearestValueIndex(
	values: readonly (number | null)[],
	target: number
): number | null {
	if (!Number.isFinite(target)) return null;

	let nearestIndex: number | null = null;
	let nearestDistance = Infinity;
	for (const [index, value] of values.entries()) {
		if (value === null || !Number.isFinite(value)) continue;
		const distance = Math.abs(value - target);
		if (distance < nearestDistance) {
			nearestIndex = index;
			nearestDistance = distance;
		}
	}
	return nearestIndex;
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
			case '10min':
			case '5min':
				return `${year}-${month}-${day} ${hours}:${minutes}`;
			default:
				return `${year}-${month}-${day}`;
		}
	});
}

export type TemporalDataBounds = {
	min: number;
	max: number;
};

/** Find the first and last timestamps that contain renderable chart data. */
export function findTemporalDataBounds<T>(
	items: readonly T[],
	getTimestamp: (item: T) => number,
	hasData: (item: T) => boolean,
	singleBucketDuration = 0
): TemporalDataBounds | null {
	let min = Infinity;
	let max = -Infinity;

	for (const item of items) {
		if (!hasData(item)) continue;
		const timestamp = getTimestamp(item);
		if (!Number.isFinite(timestamp)) continue;
		min = Math.min(min, timestamp);
		max = Math.max(max, timestamp);
	}

	if (!Number.isFinite(min) || !Number.isFinite(max)) return null;
	return min === max
		? { min: min - singleBucketDuration / 2, max: max + singleBucketDuration / 2 }
		: { min, max };
}

export function getXAxisTitle(groupBy: GroupByOption): string {
	switch (groupBy) {
		case 'date':
			return 'Date';
		case 'hour':
			return 'Hour';
		case '30min':
			return '30 Minutes';
		case '10min':
			return '10 Minutes';
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
	const colors = Array.from(
		{ length: count },
		(_, index) => `var(--chart-series-${(index % 8) + 1})`
	);
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
	if (groupBy === '10min') return 10 * 60 * 1000;
	return 5 * 60 * 1000;
}

const GROUP_BY_DETAIL_LEVEL: Record<GroupByOption, number> = {
	date: 0,
	hour: 1,
	'30min': 2,
	'10min': 3,
	'5min': 4
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
 * Click drilldown windows land on the same choices:
 * 1 day -> 5min, 7 days -> 10min, 31 days -> hour.
 * Cutoffs: 5min up to 4 days, 10min up to 8 days, 30min up to 19 days,
 * hour up to 62 days, then date.
 */
export function chooseAdaptiveGranularity(rangeMs: number): GroupByOption {
	const oneDay = 24 * 60 * 60 * 1000;
	const sevenDays = 7 * oneDay;
	const thirtyOneDays = 31 * oneDay;

	const fiveMinCutoff = (oneDay + sevenDays) / 2; // 4 days
	const tenMinCutoff = fiveMinCutoff * 2;
	const thirtyMinCutoff = (sevenDays + thirtyOneDays) / 2; // 19 days
	const hourCutoff = thirtyOneDays * 2; // 62 days

	if (rangeMs <= fiveMinCutoff) return '5min';
	if (rangeMs <= tenMinCutoff) return '10min';
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

	if (groupBy === '30min' || groupBy === '10min') {
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
	if (groupBy === '30min' || groupBy === '10min') {
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

/** Read coverage metadata from an untyped point. */
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

/** Build temporal points without compressing gaps into adjacent indexes. */
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

/** Partial buckets make both adjoining line segments visibly uncertain. */
export function isCoverageSegmentDashed(
	left: { coverage?: ChartCoverage } | null | undefined,
	right: { coverage?: ChartCoverage } | null | undefined
): boolean {
	return left?.coverage?.state === 'partial' || right?.coverage?.state === 'partial';
}
