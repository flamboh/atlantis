import { describe, expect, it, vi } from 'vitest';

vi.mock('$app/paths', () => ({
	resolve: (path: string, params: Record<string, string> = {}) =>
		path.replace(/\[(\w+)\]/g, (_, key: string) => params[key] ?? '')
}));

import {
	dateRangePresets,
	formatDateRange,
	isIsoDate
} from '../../src/lib/components/filters/date-range';
import { sumNetflowWindow } from '../../src/lib/components/netflow/netflow-window-totals';
import {
	pageDatasetId,
	pageSection,
	sectionHref
} from '../../src/lib/components/shell/app-context';
import type { NetflowStatsResult, TimeBucket } from '../../src/lib/types/types';

describe('date range helpers', () => {
	it('validates real ISO calendar dates only', () => {
		expect(isIsoDate('2025-03-01')).toBe(true);
		expect(isIsoDate('2025-02-30')).toBe(false);
		expect(isIsoDate('2025-3-1')).toBe(false);
		expect(isIsoDate('')).toBe(false);
	});

	it('formats single days, same-year and cross-year ranges', () => {
		expect(formatDateRange('2025-03-01', '2025-03-01')).toBe('Mar 1, 2025');
		expect(formatDateRange('2025-03-01', '2025-03-03')).toBe('Mar 1 – Mar 3, 2025');
		expect(formatDateRange('2024-12-30', '2025-01-02')).toBe('Dec 30, 2024 – Jan 2, 2025');
	});

	it('anchors window presets to the selected end date', () => {
		expect(dateRangePresets('2025-03-03', '2025-01-01', '2026-10-09')).toEqual([
			{ id: '1d', label: 'Last day', startDate: '2025-03-03', endDate: '2025-03-03' },
			{ id: '7d', label: 'Last 7 days', startDate: '2025-02-25', endDate: '2025-03-03' },
			{ id: '30d', label: 'Last 30 days', startDate: '2025-02-02', endDate: '2025-03-03' },
			{ id: 'all', label: 'All available', startDate: '2025-01-01', endDate: '2026-10-09' }
		]);
	});
});

describe('sumNetflowWindow', () => {
	const row = (flows: number, flowsTcp: number, packets: number, bytes: number) =>
		({ flows, flowsTcp, packets, bytes }) as NetflowStatsResult;
	const bucket = (
		state: 'complete' | 'partial' | 'unknown',
		data: NetflowStatsResult | null
	): TimeBucket<NetflowStatsResult> => ({
		bucketStart: 0,
		bucketEnd: 300,
		coverage: { state, observedUnits: 0, expectedUnits: 0 },
		data
	});

	it('adds observed buckets and counts coverage states without inventing data', () => {
		expect(
			sumNetflowWindow([
				bucket('complete', row(10, 6, 100, 1000)),
				bucket('partial', row(5, 1, 50, 400)),
				bucket('unknown', null),
				bucket('complete', row(0, 0, 0, 0))
			])
		).toEqual({
			flows: 15,
			flowsTcp: 7,
			packets: 150,
			bytes: 1400,
			buckets: 4,
			completeBuckets: 2,
			partialBuckets: 1,
			unknownBuckets: 1
		});
	});
});

describe('app shell context', () => {
	const page = (
		pathname: string,
		search = '',
		params: Record<string, string> = {},
		data: Record<string, unknown> = {}
	) => ({ params, url: new URL(`http://x${pathname}${search}`), data });

	it('maps routes to navigation sections', () => {
		expect(pageSection('/')).toBe('datasets');
		expect(pageSection('/datasets/a')).toBe('dashboard');
		expect(pageSection('/datasets/a/alerts')).toBe('alerts');
		expect(pageSection('/netflow/files/202503010200')).toBe('files');
	});

	it('resolves the dataset from params, search, then page data', () => {
		expect(pageDatasetId(page('/datasets/a', '?dataset=b', { dataset: 'a' }))).toBe('a');
		expect(pageDatasetId(page('/netflow/files/1', '?dataset=b'))).toBe('b');
		expect(pageDatasetId(page('/netflow/files', '', {}, { selectedDataset: 'c' }))).toBe('c');
		expect(pageDatasetId(page('/'))).toBeNull();
	});

	it('builds section links for a dataset', () => {
		expect(sectionHref('dashboard', 'a b')).toBe('/datasets/a b');
		expect(sectionHref('alerts', 'a')).toBe('/datasets/a/alerts');
		expect(sectionHref('files', 'a b')).toBe('/netflow/files?dataset=a%20b');
		expect(sectionHref('dashboard', null)).toBe('/');
	});
});
