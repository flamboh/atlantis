import { describe, expect, it, vi } from 'vitest';
import { GET } from '../../src/routes/api/netflow/characteristics/+server';
import {
	getDatasetDb,
	getRequestedDataset,
	listDatasetSourceDefinitions
} from '$lib/server/datasets';

vi.mock('$lib/server/datasets', () => ({
	getDatasetDb: vi.fn(),
	getRequestedDataset: vi.fn(),
	listDatasetSourceDefinitions: vi.fn()
}));

describe('/api/netflow/characteristics GET', () => {
	it('returns weighted observation averages and exact logical-source port cardinalities', async () => {
		const all = vi
			.fn()
			.mockResolvedValueOnce([
				{
					bucketStart: 100,
					bucketEnd: 200,
					ipVersion: 4,
					durationSumMs: 300,
					durationCount: 2,
					minTtlSum: 60,
					minTtlCount: 2,
					maxTtlSum: 64,
					maxTtlCount: 1
				},
				{
					bucketStart: 100,
					bucketEnd: 200,
					ipVersion: 6,
					durationSumMs: 100,
					durationCount: 2,
					minTtlSum: 0,
					minTtlCount: 0,
					maxTtlSum: 128,
					maxTtlCount: 2
				}
			])
			.mockResolvedValueOnce([
				{
					sourceId: 'uoregon_all',
					bucketStart: 100,
					bucketEnd: 200,
					ipVersion: 4,
					portSide: 'source',
					portRange: 'low',
					uniquePortCount: 7
				}
			])
			.mockResolvedValueOnce([
				{
					sourceId: 'uoregon_all',
					bucketStart: 100,
					bucketEnd: 200,
					coverageState: 'partial',
					observedUnits: 1,
					expectedUnits: 2,
					rejectedUnits: 0
				}
			])
			.mockResolvedValueOnce([
				{
					sourceId: 'uoregon_all',
					bucketStart: 100,
					bucketEnd: 200,
					coverageState: 'partial',
					observedUnits: 1,
					expectedUnits: 2,
					rejectedUnits: 0
				}
			]);
		vi.mocked(getRequestedDataset).mockResolvedValue('uoregon');
		vi.mocked(listDatasetSourceDefinitions).mockResolvedValue([
			{ sourceId: 'cc', members: ['cc'] },
			{ sourceId: 'oh', members: ['oh'] },
			{ sourceId: 'uoregon_all', members: ['cc', 'oh'] }
		]);
		vi.mocked(getDatasetDb).mockResolvedValue({ all } as never);

		const response = await GET({
			url: new URL(
				'http://localhost/api/netflow/characteristics?routers=cc,oh&granularity=1h&startDate=100&endDate=200'
			)
		} as never);

		expect(response.status).toBe(200);
		await expect(response.json()).resolves.toEqual({
			observationBuckets: [
				{
					bucketStart: 100,
					bucketEnd: 200,
					coverage: { state: 'partial', observedUnits: 1, expectedUnits: 2 },
					data: [
						{
							ipFamily: 'ipv4',
							averageDurationMs: 150,
							averageMinTtl: 30,
							averageMaxTtl: 64
						},
						{
							ipFamily: 'ipv6',
							averageDurationMs: 50,
							averageMinTtl: null,
							averageMaxTtl: 64
						},
						{
							ipFamily: 'all',
							averageDurationMs: 100,
							averageMinTtl: 30,
							averageMaxTtl: 64
						}
					]
				}
			],
			portTimelines: [
				{
					sourceId: 'uoregon_all',
					buckets: [
						{
							bucketStart: 100,
							bucketEnd: 200,
							coverage: { state: 'partial', observedUnits: 1, expectedUnits: 2 },
							data: [
								{
									ipFamily: 'ipv4',
									portSide: 'source',
									portRange: 'low',
									uniquePortCount: 7
								}
							]
						}
					]
				}
			],
			resolvedSources: ['uoregon_all']
		});
		expect(all).toHaveBeenNthCalledWith(1, expect.stringContaining('SUM(duration_sum_ms)'), [
			'uoregon_all',
			'1h',
			'all',
			'all',
			100,
			200
		]);
		expect(all).toHaveBeenNthCalledWith(2, expect.not.stringContaining('SUM(unique_port_count)'), [
			'uoregon_all',
			'1h',
			'all',
			'all',
			100,
			200
		]);
	});

	it('keeps disjoint fallback sources separate instead of summing cardinalities', async () => {
		const all = vi.fn().mockResolvedValue([]);
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		vi.mocked(listDatasetSourceDefinitions).mockResolvedValue([
			{ sourceId: 'r1', members: ['r1'] },
			{ sourceId: 'r2', members: ['r2'] }
		]);
		vi.mocked(getDatasetDb).mockResolvedValue({ all } as never);

		const response = await GET({
			url: new URL(
				'http://localhost/api/netflow/characteristics?routers=r1,r2&startDate=1&endDate=2'
			)
		} as never);

		expect(response.status).toBe(200);
		await expect(response.json()).resolves.toMatchObject({
			resolvedSources: ['r1', 'r2'],
			observationBuckets: [
				{
					bucketStart: 1,
					bucketEnd: 2,
					coverage: { state: 'unknown', observedUnits: 0, expectedUnits: 0 },
					data: null
				}
			],
			portTimelines: [
				{
					sourceId: 'r1',
					buckets: [
						{
							bucketStart: 1,
							bucketEnd: 2,
							coverage: { state: 'unknown', observedUnits: 0, expectedUnits: 0 },
							data: null
						}
					]
				},
				{
					sourceId: 'r2',
					buckets: [
						{
							bucketStart: 1,
							bucketEnd: 2,
							coverage: { state: 'unknown', observedUnits: 0, expectedUnits: 0 },
							data: null
						}
					]
				}
			]
		});
		expect(all).toHaveBeenNthCalledWith(2, expect.any(String), [
			'r1',
			'r2',
			'1h',
			'all',
			'all',
			1,
			2
		]);
	});

	it('validates the shared flow scope', async () => {
		const response = await GET({
			url: new URL(
				'http://localhost/api/netflow/characteristics?routers=r1&startDate=1&endDate=2&srcVisibility=private'
			)
		} as never);

		expect(response.status).toBe(400);
		await expect(response.json()).resolves.toEqual({
			error: 'Invalid srcVisibility. Expected one of: all, literal, anonymized'
		});
	});

	it('rejects an invalid explicit granularity instead of silently using one hour', async () => {
		const response = await GET({
			url: new URL(
				'http://localhost/api/netflow/characteristics?routers=r1&startDate=1&endDate=2&granularity=weekly'
			)
		} as never);

		expect(response.status).toBe(400);
		await expect(response.json()).resolves.toEqual({
			error: 'Invalid granularity. Expected one of: 5m, 30m, 1h, 1d'
		});
	});
});
