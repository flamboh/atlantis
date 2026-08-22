import { describe, expect, it, vi } from 'vitest';
import { buildCoverageTimelines } from '$lib/server/db/coverage';
import { dateStringToEpochPST } from '$lib/utils/timezone';

describe('buildCoverageTimelines', () => {
	it('fills a bounded timeline from explicit coverage without treating metric rows as evidence', async () => {
		const all = vi.fn().mockResolvedValue([
			{
				sourceId: 'r1',
				bucketStart: 0,
				bucketEnd: 300,
				coverageState: 'complete',
				observedUnits: 1,
				expectedUnits: 1,
				rejectedUnits: 0
			},
			{
				sourceId: 'r1',
				bucketStart: 300,
				bucketEnd: 600,
				coverageState: 'partial',
				observedUnits: 1,
				expectedUnits: 2,
				rejectedUnits: 0
			},
			{
				sourceId: 'r1',
				bucketStart: 600,
				bucketEnd: 900,
				coverageState: 'complete',
				observedUnits: 1,
				expectedUnits: 1,
				rejectedUnits: 0
			}
		]);

		const timelines = await buildCoverageTimelines({
			db: { all } as never,
			granularity: '5m',
			start: 0,
			end: 1200,
			partitions: [{ key: 'r1', sourceIds: ['r1'] }],
			rows: [
				{ bucketStart: 0, bucketEnd: 300, value: 7 },
				{ bucketStart: 300, bucketEnd: 600, value: 3 },
				{ bucketStart: 900, bucketEnd: 1200, value: 99 }
			],
			getPartitionKey: () => 'r1',
			toData: (row) => ({ value: row.value }),
			emptyData: () => ({ value: 0 })
		});

		expect(timelines.get('r1')).toEqual([
			{
				bucketStart: 0,
				bucketEnd: 300,
				coverage: { state: 'complete', observedUnits: 1, expectedUnits: 1 },
				data: { value: 7 }
			},
			{
				bucketStart: 300,
				bucketEnd: 600,
				coverage: { state: 'partial', observedUnits: 1, expectedUnits: 2 },
				data: { value: 3 }
			},
			{
				bucketStart: 600,
				bucketEnd: 900,
				coverage: { state: 'complete', observedUnits: 1, expectedUnits: 1 },
				data: { value: 0 }
			},
			{
				bucketStart: 900,
				bucketEnd: 1200,
				coverage: { state: 'unknown', observedUnits: 0, expectedUnits: 0 },
				data: null
			}
		]);
		expect(all).toHaveBeenCalledWith(expect.stringContaining('FROM bucket_coverage'), [
			'5m',
			'r1',
			0,
			1200
		]);
	});

	it('adds coverage counts when an additive metric combines disjoint sources', async () => {
		const all = vi.fn().mockResolvedValue([
			{
				sourceId: 'r1',
				bucketStart: 0,
				bucketEnd: 300,
				coverageState: 'complete',
				observedUnits: 2,
				expectedUnits: 2,
				rejectedUnits: 0
			},
			{
				sourceId: 'r2',
				bucketStart: 0,
				bucketEnd: 300,
				coverageState: 'partial',
				observedUnits: 1,
				expectedUnits: 3,
				rejectedUnits: 0
			}
		]);

		const timelines = await buildCoverageTimelines({
			db: { all } as never,
			granularity: '5m',
			start: 0,
			end: 300,
			partitions: [{ key: 'combined', sourceIds: ['r1', 'r2'] }],
			rows: [{ bucketStart: 0, value: 9 }],
			getPartitionKey: () => 'combined',
			toData: (row) => ({ value: row.value }),
			emptyData: () => ({ value: 0 })
		});

		expect(timelines.get('combined')).toEqual([
			{
				bucketStart: 0,
				bucketEnd: 300,
				coverage: { state: 'partial', observedUnits: 3, expectedUnits: 5 },
				data: { value: 9 }
			}
		]);
	});

	it('uses persisted daily bounds instead of assuming every local day is 24 hours', async () => {
		const all = vi.fn().mockResolvedValue([
			{
				sourceId: 'r1',
				bucketStart: 0,
				bucketEnd: 82_800,
				coverageState: 'complete',
				observedUnits: 276,
				expectedUnits: 276,
				rejectedUnits: 0
			},
			{
				sourceId: 'r1',
				bucketStart: 82_800,
				bucketEnd: 169_200,
				coverageState: 'complete',
				observedUnits: 288,
				expectedUnits: 288,
				rejectedUnits: 0
			}
		]);

		const timelines = await buildCoverageTimelines({
			db: { all } as never,
			granularity: '1d',
			start: 0,
			end: 169_200,
			partitions: [{ key: 'r1', sourceIds: ['r1'] }],
			rows: [],
			getPartitionKey: () => 'r1',
			toData: (row: { bucketStart: number; value: number }) => ({ value: row.value }),
			emptyData: () => ({ value: 0 })
		});

		expect(
			timelines.get('r1')?.map(({ bucketStart, bucketEnd }) => [bucketStart, bucketEnd])
		).toEqual([
			[0, 82_800],
			[82_800, 169_200]
		]);
	});

	it('preserves civil-day bounds for unknown coverage across daylight saving changes', async () => {
		const start = dateStringToEpochPST('2025-03-08');
		const transition = dateStringToEpochPST('2025-03-09');
		const afterTransition = dateStringToEpochPST('2025-03-10');
		const end = dateStringToEpochPST('2025-03-11');
		const timelines = await buildCoverageTimelines({
			db: { all: vi.fn().mockResolvedValue([]) } as never,
			granularity: '1d',
			start,
			end,
			partitions: [{ key: 'r1', sourceIds: ['r1'] }],
			rows: [],
			getPartitionKey: () => 'r1',
			toData: (row: { bucketStart: number; value: number }) => ({ value: row.value }),
			emptyData: () => ({ value: 0 })
		});

		expect(
			timelines.get('r1')?.map(({ bucketStart, bucketEnd }) => [bucketStart, bucketEnd])
		).toEqual([
			[start, transition],
			[transition, afterTransition],
			[afterTransition, end]
		]);
	});
});
