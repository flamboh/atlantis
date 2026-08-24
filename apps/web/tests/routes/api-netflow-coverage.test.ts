import { describe, expect, it, vi } from 'vitest';
import { GET } from '../../src/routes/api/netflow/coverage/+server';
import { getRequestedDataset, withDatasetDb } from '$lib/server/datasets';
import { dateStringToEpochPST } from '$lib/utils/timezone';

vi.mock('$lib/server/datasets', () => ({
	getRequestedDataset: vi.fn(),
	withDatasetDb: vi.fn()
}));

function mockDatasetSession(db: object): void {
	vi.mocked(withDatasetDb).mockImplementation(async (_datasetId, _platform, run) =>
		run({ db: db as never, listSources: async () => [], listSourceDefinitions: async () => [] })
	);
}

describe('/api/netflow/coverage GET', () => {
	it('requires at least one selected source', async () => {
		const response = await GET({
			url: new URL('http://localhost/api/netflow/coverage?startDate=1&endDate=2')
		} as never);

		expect(response.status).toBe(400);
		await expect(response.json()).resolves.toEqual({ error: 'No routers selected' });
	});

	it('returns source-separated coverage with explicit unknown gaps and no metric data', async () => {
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
				sourceId: 'r2',
				bucketStart: 0,
				bucketEnd: 300,
				coverageState: 'complete',
				observedUnits: 1,
				expectedUnits: 1,
				rejectedUnits: 0
			}
		]);
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		mockDatasetSession({ all });

		const response = await GET({
			url: new URL(
				'http://localhost/api/netflow/coverage?dataset=alpha&routers=r1,r2&groupBy=5min&startDate=0&endDate=900'
			)
		} as never);

		expect(response.status).toBe(200);
		const body = await response.json();
		expect(body).toEqual({
			timelines: [
				{
					sourceId: 'r1',
					buckets: [
						{
							bucketStart: 0,
							bucketEnd: 300,
							coverage: { state: 'complete', observedUnits: 1, expectedUnits: 1 }
						},
						{
							bucketStart: 300,
							bucketEnd: 600,
							coverage: { state: 'partial', observedUnits: 1, expectedUnits: 2 }
						},
						{
							bucketStart: 600,
							bucketEnd: 900,
							coverage: { state: 'unknown', observedUnits: 0, expectedUnits: 0 }
						}
					]
				},
				{
					sourceId: 'r2',
					buckets: [
						{
							bucketStart: 0,
							bucketEnd: 300,
							coverage: { state: 'complete', observedUnits: 1, expectedUnits: 1 }
						},
						{
							bucketStart: 300,
							bucketEnd: 600,
							coverage: { state: 'unknown', observedUnits: 0, expectedUnits: 0 }
						},
						{
							bucketStart: 600,
							bucketEnd: 900,
							coverage: { state: 'unknown', observedUnits: 0, expectedUnits: 0 }
						}
					]
				}
			],
			requestedRouters: ['r1', 'r2']
		});
		expect(all).toHaveBeenCalledWith(expect.stringContaining('FROM bucket_coverage'), [
			'5m',
			'r1',
			'r2',
			0,
			900
		]);
		expect(JSON.stringify(body)).not.toContain('data');
	});

	it('accepts explicit granularity and rejects invalid time windows', async () => {
		const invalidGranularity = await GET({
			url: new URL(
				'http://localhost/api/netflow/coverage?routers=r1&granularity=weekly&startDate=1&endDate=2'
			)
		} as never);
		const invalidRange = await GET({
			url: new URL(
				'http://localhost/api/netflow/coverage?routers=r1&granularity=1h&startDate=2&endDate=2'
			)
		} as never);

		expect(invalidGranularity.status).toBe(400);
		await expect(invalidGranularity.json()).resolves.toEqual({
			error: 'Invalid granularity. Expected one of: 5m, 30m, 1h, 1d'
		});
		expect(invalidRange.status).toBe(400);
		await expect(invalidRange.json()).resolves.toEqual({
			error: 'Start time must be before end time'
		});
	});

	it('deduplicates selected sources before querying coverage', async () => {
		const all = vi.fn().mockResolvedValue([]);
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		mockDatasetSession({ all });

		const response = await GET({
			url: new URL(
				'http://localhost/api/netflow/coverage?routers=r2,r1,r2,r1&groupBy=5min&startDate=0&endDate=600'
			)
		} as never);

		expect(response.status).toBe(200);
		await expect(response.json()).resolves.toMatchObject({
			timelines: [{ sourceId: 'r2' }, { sourceId: 'r1' }],
			requestedRouters: ['r2', 'r1']
		});
		expect(all).toHaveBeenCalledWith(expect.stringContaining('FROM bucket_coverage'), [
			'5m',
			'r1',
			'r2',
			0,
			600
		]);
	});

	it('rejects non-aligned bounds for fixed-width buckets', async () => {
		const response = await GET({
			url: new URL(
				'http://localhost/api/netflow/coverage?routers=r1&groupBy=5min&startDate=1&endDate=600'
			)
		} as never);

		expect(response.status).toBe(400);
		await expect(response.json()).resolves.toEqual({
			error: 'Start and end times must align to 5m bucket boundaries'
		});
	});

	it('requires daily bounds to be Pacific local midnights', async () => {
		const start = dateStringToEpochPST('2025-03-09');
		const end = dateStringToEpochPST('2025-03-10');
		const response = await GET({
			url: new URL(
				`http://localhost/api/netflow/coverage?routers=r1&groupBy=date&startDate=${start + 3600}&endDate=${end}`
			)
		} as never);

		expect(response.status).toBe(400);
		await expect(response.json()).resolves.toEqual({
			error: 'Start and end times must align to 1d bucket boundaries'
		});
	});
});
