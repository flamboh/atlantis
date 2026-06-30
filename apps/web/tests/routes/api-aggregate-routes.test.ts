import { describe, expect, it, vi } from 'vitest';
import { GET as getRouters } from '../../src/routes/api/routers/+server';
import { GET as getIpStats } from '../../src/routes/api/ip/stats/+server';
import { GET as getProtocolStats } from '../../src/routes/api/protocol/stats/+server';
import { GET as getSpectrumStats } from '../../src/routes/api/netflow/spectrum-stats/+server';
import { GET as getStructureStats } from '../../src/routes/api/netflow/structure-stats/+server';
import { getDatasetDb, getRequestedDataset, listDatasetSources } from '$lib/server/datasets';

vi.mock('$lib/server/datasets', () => ({
	getDatasetDb: vi.fn(),
	getRequestedDataset: vi.fn(),
	listDatasetSources: vi.fn()
}));

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
		const all = vi.fn().mockResolvedValue([
			{
				router: 'r1',
				bucketStart: 100,
				bucketEnd: 200,
				saIpv4Count: 3,
				daIpv4Count: 4,
				saIpv6Count: 5,
				daIpv6Count: 6,
				processedAt: 'now'
			}
		]);
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		vi.mocked(getDatasetDb).mockResolvedValue({
			all
		} as never);

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
			buckets: [
				{
					router: 'r1',
					bucketStart: 100,
					bucketEnd: 200,
					saIpv4Count: 3,
					daIpv4Count: 4,
					saIpv6Count: 5,
					daIpv6Count: 6,
					processedAt: 'now',
					granularity: '5m'
				}
			],
			availableGranularities: ['5m', '30m', '1h', '1d'],
			requestedRouters: ['r1']
		});
	});

	it('keeps selected unique-count sources separate', async () => {
		const all = vi.fn().mockResolvedValue([]);
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		vi.mocked(getDatasetDb).mockResolvedValue({
			all
		} as never);

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
		await expect(response.json()).resolves.toEqual({
			buckets: [],
			availableGranularities: ['5m', '30m', '1h', '1d'],
			requestedRouters: ['cc_ir1_gw', 'oh_ir1_gw', 'uoregon_all']
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

	it('keeps selected protocol sources separate', async () => {
		const all = vi.fn().mockResolvedValue([]);
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		vi.mocked(getDatasetDb).mockResolvedValue({
			all
		} as never);

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
		await expect(response.json()).resolves.toEqual({
			buckets: [],
			availableGranularities: ['5m', '30m', '1h', '1d'],
			requestedRouters: ['cc_ir1_gw', 'oh_ir1_gw', 'uoregon_all']
		});
	});

	it('parses spectrum and structure json payloads, tolerating bad json', async () => {
		const all = vi
			.fn()
			.mockResolvedValueOnce([
				{
					router: 'r1',
					bucketStart: 100,
					addressSide: 'source',
					valuesJson: '[{"alpha":1,"f":2}]'
				},
				{
					router: 'r1',
					bucketStart: 100,
					addressSide: 'destination',
					valuesJson: 'not-json'
				}
			])
			.mockResolvedValueOnce([
				{
					router: 'r1',
					bucketStart: 100,
					addressSide: 'source',
					valuesJson: '[{"q":1,"tauTilde":2,"sd":0.5}]'
				},
				{
					router: 'r1',
					bucketStart: 100,
					addressSide: 'destination',
					valuesJson: 'not-json'
				}
			]);
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		vi.mocked(getDatasetDb).mockResolvedValue({
			all
		} as never);

		const spectrumResponse = await getSpectrumStats({
			url: new URL(
				'http://localhost/api/netflow/spectrum-stats?routers=r1&startDate=100&endDate=200'
			)
		} as never);
		const structureResponse = await getStructureStats({
			url: new URL(
				'http://localhost/api/netflow/structure-stats?routers=r1&startDate=100&endDate=200'
			)
		} as never);

		await expect(spectrumResponse.json()).resolves.toEqual({
			buckets: [
				{ bucketStart: 100, router: 'r1', spectrumSa: [{ alpha: 1, f: 2 }], spectrumDa: [] }
			],
			requestedRouters: ['r1']
		});
		await expect(structureResponse.json()).resolves.toEqual({
			buckets: [
				{
					bucketStart: 100,
					router: 'r1',
					structureSa: [{ q: 1, tau: 2, sd: 0.5 }],
					structureDa: []
				}
			],
			requestedRouters: ['r1']
		});
	});

	it('keeps selected spectrum sources separate', async () => {
		const all = vi.fn().mockResolvedValue([]);
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		vi.mocked(getDatasetDb).mockResolvedValue({
			all
		} as never);

		const response = await getSpectrumStats({
			url: new URL(
				'http://localhost/api/netflow/spectrum-stats?routers=cc_ir1_gw,oh_ir1_gw,uoregon_all&granularity=1h&startDate=100&endDate=200'
			)
		} as never);

		expect(response.status).toBe(200);
		expect(all).toHaveBeenCalledWith(expect.stringContaining('FROM address_structure_stats'), [
			'1h',
			'cc_ir1_gw',
			'oh_ir1_gw',
			'uoregon_all',
			'all',
			'all',
			100,
			200
		]);
		await expect(response.json()).resolves.toEqual({
			buckets: [],
			requestedRouters: ['cc_ir1_gw', 'oh_ir1_gw', 'uoregon_all']
		});
	});
});
