import { describe, expect, it, vi } from 'vitest';
import { GET as getRouters } from '../../src/routes/api/routers/+server';
import { GET as getIpStats } from '../../src/routes/api/ip/stats/+server';
import { GET as getProtocolStats } from '../../src/routes/api/protocol/stats/+server';
import { GET as getSpectrumStats } from '../../src/routes/api/netflow/spectrum-stats/+server';
import { GET as getStructureStats } from '../../src/routes/api/netflow/structure-stats/+server';
import { getRequestedDataset, listDatasetSources, withDatasetDb } from '$lib/server/datasets';

vi.mock('$lib/server/datasets', () => ({
	getRequestedDataset: vi.fn(),
	listDatasetSources: vi.fn(),
	withDatasetDb: vi.fn()
}));

function mockDatasetSession(db: object): void {
	vi.mocked(withDatasetDb).mockImplementation(async (_datasetId, _platform, run) =>
		run({ db: db as never, listSources: async () => [], listSourceDefinitions: async () => [] })
	);
}

const DEFAULT_Q_GRID = { ipVersion: 4, qMin: -0.5, qStep: 0.125, qCount: 33 };

function f32(values: number[]): Buffer {
	return Buffer.from(Float32Array.from(values).buffer);
}

describe('aggregate API routes', () => {
	it('lists routers for a dataset and returns 404 when none exist', async () => {
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		vi.mocked(listDatasetSources).mockResolvedValueOnce(['r1', 'r2']).mockResolvedValueOnce([]);

		const okResponse = await getRouters({
			url: new URL('http://localhost/api/routers?dataset=alpha')
		} as never);
		const emptyResponse = await getRouters({
			url: new URL('http://localhost/api/routers?dataset=alpha')
		} as never);

		await expect(okResponse.json()).resolves.toEqual(['r1', 'r2']);
		expect(emptyResponse.status).toBe(404);
		await expect(emptyResponse.json()).resolves.toEqual({
			error: "No routers available for dataset 'alpha'"
		});
	});

	it('validates ip stats requests and returns grouped data', async () => {
		const all = vi
			.fn()
			.mockResolvedValueOnce([
				{
					router: 'r1',
					bucketStart: 100,
					bucketEnd: 200,
					saIpv4Count: 3,
					daIpv4Count: 4,
					saIpv6Count: 5,
					daIpv6Count: 6
				}
			])
			.mockResolvedValueOnce([
				{
					sourceId: 'r1',
					bucketStart: 100,
					bucketEnd: 200,
					coverageState: 'complete',
					observedUnits: 1,
					expectedUnits: 1,
					rejectedUnits: 0
				}
			]);
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		mockDatasetSession({
			all
		});

		const badResponse = await getIpStats({
			url: new URL('http://localhost/api/ip/stats?routers=&startDate=1&endDate=2')
		} as never);
		const okResponse = await getIpStats({
			url: new URL(
				'http://localhost/api/ip/stats?routers=r1&granularity=5m&startDate=100&endDate=200'
			)
		} as never);

		expect(badResponse.status).toBe(400);
		await expect(okResponse.json()).resolves.toEqual({
			timelines: [
				{
					router: 'r1',
					buckets: [
						{
							bucketStart: 100,
							bucketEnd: 200,
							coverage: { state: 'complete', observedUnits: 1, expectedUnits: 1 },
							data: {
								saIpv4Count: 3,
								daIpv4Count: 4,
								saIpv6Count: 5,
								daIpv6Count: 6
							}
						}
					]
				}
			]
		});
	});

	it('keeps selected unique-count sources separate', async () => {
		const all = vi.fn().mockResolvedValueOnce([]).mockResolvedValueOnce([]);
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		mockDatasetSession({
			all
		});

		const response = await getIpStats({
			url: new URL(
				'http://localhost/api/ip/stats?routers=cc_ir1_gw,oh_ir1_gw,uoregon_all&granularity=1h&startDate=100&endDate=200'
			)
		} as never);

		expect(response.status).toBe(200);
		expect(all).toHaveBeenCalledWith(expect.stringContaining('FROM address_count_stats'), [
			'1h',
			'cc_ir1_gw',
			'oh_ir1_gw',
			'uoregon_all',
			'all',
			'all',
			100,
			200
		]);
		await expect(response.json()).resolves.toMatchObject({
			timelines: [
				{
					router: 'cc_ir1_gw',
					buckets: [
						{
							bucketStart: 100,
							bucketEnd: 200,
							coverage: { state: 'unknown', observedUnits: 0, expectedUnits: 0 },
							data: null
						}
					]
				},
				{
					router: 'oh_ir1_gw',
					buckets: [
						{
							bucketStart: 100,
							bucketEnd: 200,
							coverage: { state: 'unknown', observedUnits: 0, expectedUnits: 0 },
							data: null
						}
					]
				},
				{
					router: 'uoregon_all',
					buckets: [
						{
							bucketStart: 100,
							bucketEnd: 200,
							coverage: { state: 'unknown', observedUnits: 0, expectedUnits: 0 },
							data: null
						}
					]
				}
			]
		});
	});

	it('maps protocol unknown-dataset errors to 400', async () => {
		vi.mocked(getRequestedDataset).mockImplementation(async () => {
			throw new Error("Unknown dataset 'bad'");
		});

		const response = await getProtocolStats({
			url: new URL('http://localhost/api/protocol/stats?routers=r1&startDate=100&endDate=200')
		} as never);

		expect(response.status).toBe(400);
		await expect(response.json()).resolves.toEqual({ error: "Unknown dataset 'bad'" });
	});

	it('returns protocol payloads inside router timelines', async () => {
		const all = vi
			.fn()
			.mockResolvedValueOnce([
				{
					router: 'r1',
					bucketStart: 100,
					bucketEnd: 200,
					uniqueProtocolsIpv4: 3,
					uniqueProtocolsIpv6: 4
				}
			])
			.mockResolvedValueOnce([
				{
					sourceId: 'r1',
					bucketStart: 100,
					bucketEnd: 200,
					coverageState: 'complete',
					observedUnits: 1,
					expectedUnits: 1,
					rejectedUnits: 0
				}
			]);
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		mockDatasetSession({ all });

		const response = await getProtocolStats({
			url: new URL(
				'http://localhost/api/protocol/stats?routers=r1&granularity=1h&startDate=100&endDate=200'
			)
		} as never);

		await expect(response.json()).resolves.toEqual({
			timelines: [
				{
					router: 'r1',
					buckets: [
						{
							bucketStart: 100,
							bucketEnd: 200,
							coverage: { state: 'complete', observedUnits: 1, expectedUnits: 1 },
							data: {
								uniqueProtocolsIpv4: 3,
								uniqueProtocolsIpv6: 4
							}
						}
					]
				}
			]
		});
	});

	it('keeps selected protocol sources separate', async () => {
		const all = vi.fn().mockResolvedValueOnce([]).mockResolvedValueOnce([]);
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		mockDatasetSession({
			all
		});

		const response = await getProtocolStats({
			url: new URL(
				'http://localhost/api/protocol/stats?routers=cc_ir1_gw,oh_ir1_gw,uoregon_all&granularity=1h&startDate=100&endDate=200'
			)
		} as never);

		expect(response.status).toBe(200);
		expect(all).toHaveBeenCalledWith(expect.stringContaining('FROM protocol_stats'), [
			'1h',
			'cc_ir1_gw',
			'oh_ir1_gw',
			'uoregon_all',
			'all',
			'all',
			100,
			200
		]);
		await expect(response.json()).resolves.toMatchObject({
			timelines: [
				{
					router: 'cc_ir1_gw',
					buckets: [
						{
							bucketStart: 100,
							bucketEnd: 200,
							coverage: { state: 'unknown', observedUnits: 0, expectedUnits: 0 },
							data: null
						}
					]
				},
				{
					router: 'oh_ir1_gw',
					buckets: [
						{
							bucketStart: 100,
							bucketEnd: 200,
							coverage: { state: 'unknown', observedUnits: 0, expectedUnits: 0 },
							data: null
						}
					]
				},
				{
					router: 'uoregon_all',
					buckets: [
						{
							bucketStart: 100,
							bucketEnd: 200,
							coverage: { state: 'unknown', observedUnits: 0, expectedUnits: 0 },
							data: null
						}
					]
				}
			]
		});
	});

	it('decodes spectrum and structure MAAD blobs into points', async () => {
		const spectrumAll = vi
			.fn()
			.mockResolvedValueOnce([
				{
					router: 'r1',
					bucketStart: 100,
					bucketEnd: 200,
					saSpectrum: f32([1, 2]),
					daSpectrum: null
				},
				{
					router: 'r1',
					bucketStart: 200,
					bucketEnd: 500,
					saSpectrum: f32([]),
					daSpectrum: null
				}
			])
			.mockResolvedValueOnce([
				{
					sourceId: 'r1',
					bucketStart: 100,
					bucketEnd: 200,
					coverageState: 'complete',
					observedUnits: 1,
					expectedUnits: 1,
					rejectedUnits: 0
				},
				{
					sourceId: 'r1',
					bucketStart: 200,
					bucketEnd: 500,
					coverageState: 'complete',
					observedUnits: 1,
					expectedUnits: 1,
					rejectedUnits: 0
				}
			]);
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		mockDatasetSession({ all: spectrumAll });

		const spectrumResponse = await getSpectrumStats({
			url: new URL(
				'http://localhost/api/netflow/spectrum-stats?routers=r1&startDate=100&endDate=500'
			)
		} as never);

		await expect(spectrumResponse.json()).resolves.toEqual({
			timelines: [
				{
					router: 'r1',
					buckets: [
						{
							bucketStart: 100,
							bucketEnd: 200,
							coverage: { state: 'complete', observedUnits: 1, expectedUnits: 1 },
							data: {
								spectrumSa: [{ alpha: Math.fround(1), f: Math.fround(2) }],
								spectrumDa: []
							}
						},
						{
							bucketStart: 200,
							bucketEnd: 500,
							coverage: { state: 'complete', observedUnits: 1, expectedUnits: 1 },
							data: {
								spectrumSa: [],
								spectrumDa: []
							}
						}
					]
				}
			],
			requestedRouters: ['r1']
		});

		const structureGet = vi.fn().mockResolvedValue(DEFAULT_Q_GRID);
		const structureAll = vi
			.fn()
			.mockResolvedValueOnce([
				{
					router: 'r1',
					bucketStart: 100,
					bucketEnd: 200,
					saTau: f32([0.2, 0.4]),
					saTauSd: f32([0.01, 0.02]),
					daTau: null,
					daTauSd: null
				},
				{
					router: 'r1',
					bucketStart: 200,
					bucketEnd: 500,
					saTau: f32([]),
					saTauSd: f32([]),
					daTau: null,
					daTauSd: null
				}
			])
			.mockResolvedValueOnce([
				{
					sourceId: 'r1',
					bucketStart: 100,
					bucketEnd: 200,
					coverageState: 'complete',
					observedUnits: 1,
					expectedUnits: 1,
					rejectedUnits: 0
				},
				{
					sourceId: 'r1',
					bucketStart: 200,
					bucketEnd: 500,
					coverageState: 'complete',
					observedUnits: 1,
					expectedUnits: 1,
					rejectedUnits: 0
				}
			]);
		mockDatasetSession({ all: structureAll, get: structureGet });

		const structureResponse = await getStructureStats({
			url: new URL(
				'http://localhost/api/netflow/structure-stats?routers=r1&startDate=100&endDate=500'
			)
		} as never);

		await expect(structureResponse.json()).resolves.toEqual({
			timelines: [
				{
					router: 'r1',
					buckets: [
						{
							bucketStart: 100,
							bucketEnd: 200,
							coverage: { state: 'complete', observedUnits: 1, expectedUnits: 1 },
							data: {
								structureSa: [
									{ q: -0.5, tau: Math.fround(0.2), sd: Math.fround(0.01) },
									{ q: -0.375, tau: Math.fround(0.4), sd: Math.fround(0.02) }
								],
								structureDa: []
							}
						},
						{
							bucketStart: 200,
							bucketEnd: 500,
							coverage: { state: 'complete', observedUnits: 1, expectedUnits: 1 },
							data: {
								structureSa: [],
								structureDa: []
							}
						}
					]
				}
			],
			requestedRouters: ['r1']
		});
	});

	it('keeps selected spectrum sources separate', async () => {
		const all = vi.fn().mockResolvedValue([]);
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		mockDatasetSession({
			all
		});

		const response = await getSpectrumStats({
			url: new URL(
				'http://localhost/api/netflow/spectrum-stats?routers=cc_ir1_gw,oh_ir1_gw,uoregon_all&granularity=1h&startDate=100&endDate=200'
			)
		} as never);

		expect(response.status).toBe(200);
		expect(all).toHaveBeenCalledWith(expect.stringContaining('FROM address_maad_stats'), [
			'1h',
			'cc_ir1_gw',
			'oh_ir1_gw',
			'uoregon_all',
			'all',
			'all',
			100,
			200,
			4,
			'addresses'
		]);
		await expect(response.json()).resolves.toEqual({
			timelines: [
				{
					router: 'cc_ir1_gw',
					buckets: [
						{
							bucketStart: 100,
							bucketEnd: 200,
							coverage: { state: 'unknown', observedUnits: 0, expectedUnits: 0 },
							data: null
						}
					]
				},
				{
					router: 'oh_ir1_gw',
					buckets: [
						{
							bucketStart: 100,
							bucketEnd: 200,
							coverage: { state: 'unknown', observedUnits: 0, expectedUnits: 0 },
							data: null
						}
					]
				},
				{
					router: 'uoregon_all',
					buckets: [
						{
							bucketStart: 100,
							bucketEnd: 200,
							coverage: { state: 'unknown', observedUnits: 0, expectedUnits: 0 },
							data: null
						}
					]
				}
			],
			requestedRouters: ['cc_ir1_gw', 'oh_ir1_gw', 'uoregon_all']
		});
	});

	it('binds the requested MAAD ip version for structure and spectrum stats', async () => {
		const all = vi.fn().mockResolvedValue([]);
		const get = vi.fn().mockResolvedValue(DEFAULT_Q_GRID);
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		mockDatasetSession({ all, get });

		const structureResponse = await getStructureStats({
			url: new URL(
				'http://localhost/api/netflow/structure-stats?routers=r1&startDate=100&endDate=200&ipVersion=6'
			)
		} as never);
		expect(structureResponse.status).toBe(200);
		expect(all).toHaveBeenNthCalledWith(1, expect.stringContaining('AND ip_version = ?'), [
			'1h',
			'r1',
			'all',
			'all',
			100,
			200,
			6,
			'addresses'
		]);

		const spectrumResponse = await getSpectrumStats({
			url: new URL(
				'http://localhost/api/netflow/spectrum-stats?routers=r1&startDate=100&endDate=200&ipVersion=6'
			)
		} as never);
		expect(spectrumResponse.status).toBe(200);
		expect(all).toHaveBeenNthCalledWith(3, expect.stringContaining('AND ip_version = ?'), [
			'1h',
			'r1',
			'all',
			'all',
			100,
			200,
			6,
			'addresses'
		]);
	});

	it('rejects an invalid MAAD ip version for structure and spectrum stats', async () => {
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');

		const structureResponse = await getStructureStats({
			url: new URL(
				'http://localhost/api/netflow/structure-stats?routers=r1&startDate=100&endDate=200&ipVersion=5'
			)
		} as never);
		expect(structureResponse.status).toBe(400);
		await expect(structureResponse.json()).resolves.toEqual({
			error: 'Invalid ipVersion. Expected one of: 4, 6'
		});

		const spectrumResponse = await getSpectrumStats({
			url: new URL(
				'http://localhost/api/netflow/spectrum-stats?routers=r1&startDate=100&endDate=200&ipVersion=5'
			)
		} as never);
		expect(spectrumResponse.status).toBe(400);
		await expect(spectrumResponse.json()).resolves.toEqual({
			error: 'Invalid ipVersion. Expected one of: 4, 6'
		});
	});

	it('filters structure stats by the requested MAAD measure', async () => {
		const all = vi.fn().mockResolvedValue([]);
		const get = vi.fn().mockResolvedValue(DEFAULT_Q_GRID);
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		mockDatasetSession({ all, get });

		for (const measure of ['addresses', 'packets', 'bytes']) {
			all.mockClear();
			const response = await getStructureStats({
				url: new URL(
					`http://localhost/api/netflow/structure-stats?routers=r1&startDate=100&endDate=200&measure=${measure}`
				)
			} as never);
			expect(response.status).toBe(200);
			expect(all).toHaveBeenNthCalledWith(1, expect.stringContaining('AND measure = ?'), [
				'1h',
				'r1',
				'all',
				'all',
				100,
				200,
				4,
				measure
			]);
		}
	});

	it('rejects unknown measures and weighted spectrum requests', async () => {
		const all = vi.fn().mockResolvedValue([]);
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		mockDatasetSession({ all });

		const invalid = await getStructureStats({
			url: new URL(
				'http://localhost/api/netflow/structure-stats?routers=r1&startDate=100&endDate=200&measure=flows'
			)
		} as never);
		expect(invalid.status).toBe(400);
		await expect(invalid.json()).resolves.toEqual({
			error: 'Invalid measure. Expected one of: addresses, packets, bytes'
		});

		const weightedSpectrum = await getSpectrumStats({
			url: new URL(
				'http://localhost/api/netflow/spectrum-stats?routers=r1&startDate=100&endDate=200&measure=packets'
			)
		} as never);
		expect(weightedSpectrum.status).toBe(400);
		await expect(weightedSpectrum.json()).resolves.toEqual({
			error: 'Spectrum is only computed for the addresses measure, not packets'
		});
		expect(all).not.toHaveBeenCalled();
	});
});
