import { describe, expect, it, vi } from 'vitest';
import { GET } from '../../src/routes/api/netflow/stats/+server';
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

describe('/api/netflow/stats GET', () => {
	it('returns 400 when no routers are selected', async () => {
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');

		const response = await GET({
			url: new URL('http://localhost/api/netflow/stats?startDate=1&endDate=2')
		} as never);

		expect(response.status).toBe(400);
		await expect(response.json()).resolves.toEqual({ error: 'No routers selected' });
	});

	it('returns normalized bucket rows from the database', async () => {
		const all = vi.fn().mockResolvedValue([
			{
				bucketStart: 100,
				flows: 1,
				flowsTcp: null,
				flowsUdp: 2,
				flowsIcmp: 3,
				flowsOther: 4,
				packets: 5,
				packetsTcp: 6,
				packetsUdp: 7,
				packetsIcmp: 8,
				packetsOther: 9,
				bytes: 10,
				bytesTcp: 11,
				bytesUdp: 12,
				bytesIcmp: 13,
				bytesOther: 14,
				flowsIpv4: 15,
				flowsTcpIpv4: 16,
				flowsUdpIpv4: 17,
				flowsIcmpIpv4: 18,
				flowsOtherIpv4: 19,
				packetsIpv4: 20,
				packetsTcpIpv4: 21,
				packetsUdpIpv4: 22,
				packetsIcmpIpv4: 23,
				packetsOtherIpv4: 24,
				bytesIpv4: 25,
				bytesTcpIpv4: 26,
				bytesUdpIpv4: 27,
				bytesIcmpIpv4: 28,
				bytesOtherIpv4: 29,
				flowsIpv6: 30,
				flowsTcpIpv6: 31,
				flowsUdpIpv6: 32,
				flowsIcmpIpv6: 33,
				flowsOtherIpv6: 34,
				packetsIpv6: 35,
				packetsTcpIpv6: 36,
				packetsUdpIpv6: 37,
				packetsIcmpIpv6: 38,
				packetsOtherIpv6: 39,
				bytesIpv6: 40,
				bytesTcpIpv6: 41,
				bytesUdpIpv6: 42,
				bytesIcmpIpv6: 43,
				bytesOtherIpv6: 44
			}
		]);
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		vi.mocked(listDatasetSourceDefinitions).mockResolvedValue([
			{ sourceId: 'r1', members: ['r1'] },
			{ sourceId: 'r2', members: ['r2'] }
		]);
		vi.mocked(getDatasetDb).mockResolvedValue({
			all
		} as never);

		const response = await GET({
			url: new URL(
				'http://localhost/api/netflow/stats?routers=r1,r2&startDate=1&endDate=2&groupBy=hour'
			)
		} as never);

		expect(response.status).toBe(200);
		await expect(response.json()).resolves.toEqual({
			result: [
				{
					bucketStart: 100,
					flows: 1,
					flowsTcp: 0,
					flowsUdp: 2,
					flowsIcmp: 3,
					flowsOther: 4,
					packets: 5,
					packetsTcp: 6,
					packetsUdp: 7,
					packetsIcmp: 8,
					packetsOther: 9,
					bytes: 10,
					bytesTcp: 11,
					bytesUdp: 12,
					bytesIcmp: 13,
					bytesOther: 14,
					flowsIpv4: 15,
					flowsTcpIpv4: 16,
					flowsUdpIpv4: 17,
					flowsIcmpIpv4: 18,
					flowsOtherIpv4: 19,
					packetsIpv4: 20,
					packetsTcpIpv4: 21,
					packetsUdpIpv4: 22,
					packetsIcmpIpv4: 23,
					packetsOtherIpv4: 24,
					bytesIpv4: 25,
					bytesTcpIpv4: 26,
					bytesUdpIpv4: 27,
					bytesIcmpIpv4: 28,
					bytesOtherIpv4: 29,
					flowsIpv6: 30,
					flowsTcpIpv6: 31,
					flowsUdpIpv6: 32,
					flowsIcmpIpv6: 33,
					flowsOtherIpv6: 34,
					packetsIpv6: 35,
					packetsTcpIpv6: 36,
					packetsUdpIpv6: 37,
					packetsIcmpIpv6: 38,
					packetsOtherIpv6: 39,
					bytesIpv6: 40,
					bytesTcpIpv6: 41,
					bytesUdpIpv6: 42,
					bytesIcmpIpv6: 43,
					bytesOtherIpv6: 44
				}
			],
			availableIpFamilies: ['all', 'ipv4', 'ipv6']
		});
		expect(all).toHaveBeenCalledWith(expect.stringContaining('FROM traffic_stats_v3'), [
			'r1',
			'r2',
			'1h',
			'all',
			'all',
			1,
			2
		]);
		expect(all).toHaveBeenCalledWith(
			expect.stringContaining('AND granularity = ?'),
			expect.any(Array)
		);
		expect(all).toHaveBeenCalledWith(
			expect.stringContaining('CASE WHEN ip_version = 4'),
			expect.any(Array)
		);
	});

	it('uses v3 5-minute stats for 5-minute requests', async () => {
		const all = vi.fn().mockResolvedValue([{ bucketStart: 100, flows: 1 }]);
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		vi.mocked(listDatasetSourceDefinitions).mockResolvedValue([
			{ sourceId: 'r1', members: ['r1'] }
		]);
		vi.mocked(getDatasetDb).mockResolvedValue({
			all
		} as never);

		const response = await GET({
			url: new URL(
				'http://localhost/api/netflow/stats?routers=r1&startDate=1&endDate=2&groupBy=5min'
			)
		} as never);

		expect(response.status).toBe(200);
		expect(all).toHaveBeenCalledWith(expect.stringContaining('FROM traffic_stats_v3'), [
			'r1',
			'5m',
			'all',
			'all',
			1,
			2
		]);
	});

	it('uses an exact union source for additive totals', async () => {
		const all = vi.fn().mockResolvedValue([{ bucketStart: 100, flows: 5 }]);
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		vi.mocked(listDatasetSourceDefinitions).mockResolvedValue([
			{ sourceId: 'cc_ir1_gw', members: ['cc_ir1_gw'] },
			{ sourceId: 'oh_ir1_gw', members: ['oh_ir1_gw'] },
			{ sourceId: 'uoregon_all', members: ['cc_ir1_gw', 'oh_ir1_gw'] }
		]);
		vi.mocked(getDatasetDb).mockResolvedValue({
			all
		} as never);

		const response = await GET({
			url: new URL(
				'http://localhost/api/netflow/stats?routers=cc_ir1_gw,oh_ir1_gw,uoregon_all&startDate=1&endDate=2&groupBy=hour'
			)
		} as never);

		expect(response.status).toBe(200);
		expect(all).toHaveBeenCalledWith(expect.stringContaining('FROM traffic_stats_v3'), [
			'uoregon_all',
			'1h',
			'all',
			'all',
			1,
			2
		]);
	});

	it('returns 500 when the database query fails', async () => {
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		vi.mocked(listDatasetSourceDefinitions).mockResolvedValue([
			{ sourceId: 'r1', members: ['r1'] }
		]);
		vi.mocked(getDatasetDb).mockResolvedValue({
			all: vi.fn(() => {
				throw new Error('boom');
			})
		} as never);

		const response = await GET({
			url: new URL('http://localhost/api/netflow/stats?routers=r1&startDate=1&endDate=2')
		} as never);

		expect(response.status).toBe(500);
		await expect(response.json()).resolves.toEqual({ error: 'Database query failed' });
	});
});
