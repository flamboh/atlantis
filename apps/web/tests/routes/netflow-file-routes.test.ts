import { describe, expect, it, vi } from 'vitest';
import { createDateFromPSTComponents } from '../../src/lib/utils/timezone';
import {
	getDatasetFromRequest,
	getNetflowFilePath,
	slugToBucketStart
} from '../../src/routes/api/netflow/files/[slug]/utils';
import { GET as getIpCounts } from '../../src/routes/api/netflow/files/[slug]/ip-counts/+server';
import { GET as getSpectrum } from '../../src/routes/api/netflow/files/[slug]/spectrum/+server';
import { GET as getStructure } from '../../src/routes/api/netflow/files/[slug]/structure/+server';
import { GET as getDetails } from '../../src/routes/api/netflow/files/[slug]/details/+server';
import { GET as getSingularities } from '../../src/routes/api/netflow/files/[slug]/singularities/+server';
import { getRequestedDataset, getDatasetDb } from '$lib/server/datasets';

vi.mock('$lib/server/datasets', () => ({
	getRequestedDataset: vi.fn(),
	getDatasetDb: vi.fn()
}));

describe('netflow file helpers and routes', () => {
	it('parses slugs and resolves dataset from requests', async () => {
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');

		expect(slugToBucketStart('202503010005')).toBe(
			Math.floor(createDateFromPSTComponents(2025, 3, 1, 0, 5).getTime() / 1000)
		);
		expect(slugToBucketStart('bad')).toBeNull();
		await expect(
			getDatasetFromRequest(new URL('http://localhost/api?dataset=alpha'))
		).resolves.toBe('alpha');
	});

	it('looks up netflow file paths by slug + router', async () => {
		vi.mocked(getDatasetDb).mockResolvedValue({
			get: vi.fn().mockResolvedValue({ input_locator: '/captures/r1/nfcapd.202503010005' })
		} as never);

		await expect(getNetflowFilePath(undefined, '202503010005', 'r1')).resolves.toBe(
			'/captures/r1/nfcapd.202503010005'
		);
	});

	it('returns ip/spectrum/structure payloads and validation errors', async () => {
		const get = vi
			.fn()
			.mockResolvedValueOnce({
				saIpv4Count: 1,
				daIpv4Count: 2,
				saIpv6Count: 3,
				daIpv6Count: 4
			})
			.mockResolvedValueOnce({
				valuesJson: '[{"alpha":3,"f":4}]'
			})
			.mockResolvedValueOnce({
				valuesJson: '[{"q":1,"tauTilde":2,"sd":0.5}]'
			});
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		vi.mocked(getDatasetDb).mockResolvedValue({
			get
		} as never);

		const badIpResponse = await getIpCounts({
			params: { slug: 'bad' },
			url: new URL('http://localhost/api/netflow/files/bad/ip-counts')
		} as never);
		const ipResponse = await getIpCounts({
			params: { slug: '202503010005' },
			url: new URL('http://localhost/api/netflow/files/x/ip-counts?router=r1&source=true')
		} as never);
		const spectrumResponse = await getSpectrum({
			params: { slug: '202503010005' },
			url: new URL('http://localhost/api/netflow/files/x/spectrum?router=r1&source=false')
		} as never);
		const structureResponse = await getStructure({
			params: { slug: '202503010005' },
			url: new URL('http://localhost/api/netflow/files/x/structure?router=r1&source=true')
		} as never);

		expect(badIpResponse.status).toBe(400);
		await expect(ipResponse.json()).resolves.toEqual({ ipv4Count: 1, ipv6Count: 3 });
		await expect(spectrumResponse.json()).resolves.toEqual({
			slug: '202503010005',
			router: 'r1',
			filename: 'nfcapd.202503010005',
			spectrum: [{ alpha: 3, f: 4 }],
			metadata: {
				dataSource: 'Database: spectrum_stats 5m bucket (Destination Addresses)',
				uniqueIPCount: -1,
				pointCount: 1,
				addressType: 'Destination',
				alphaRange: { min: 3, max: 3 }
			}
		});
		await expect(structureResponse.json()).resolves.toEqual({
			slug: '202503010005',
			router: 'r1',
			filename: 'nfcapd.202503010005',
			structureFunction: [{ q: 1, tau: 2, sd: 0.5 }],
			metadata: {
				dataSource: 'Database: structure_stats 5m bucket (Source Addresses)',
				uniqueIPCount: -1,
				pointCount: 1,
				addressType: 'Source',
				qRange: { min: 1, max: 1 }
			}
		});
	});

	it('builds file details response with attached derived datasets', async () => {
		vi.mocked(getRequestedDataset).mockResolvedValue('alpha');
		vi.mocked(getDatasetDb).mockResolvedValue({
			all: vi.fn().mockResolvedValue([
				{
					router: 'r1',
					file_path: '/captures/r1/nfcapd.202503010005',
					input_kind: 'nfcapd',
					input_status: 'processed',
					input_error_message: null,
					bucket_start: 1740823500,
					bucket_end: 1740823800,
					flows: 1,
					flows_tcp: 2,
					flows_udp: 3,
					flows_icmp: 4,
					flows_other: 5,
					packets: 6,
					packets_tcp: 7,
					packets_udp: 8,
					packets_icmp: 9,
					packets_other: 10,
					bytes: 11,
					bytes_tcp: 12,
					bytes_udp: 13,
					bytes_icmp: 14,
					bytes_other: 15,
					first_timestamp: 16,
					last_timestamp: 17,
					msec_first: 18,
					msec_last: 19,
					sequence_failures: 20,
					processed_at: 'now',
					saIpv4Count: 2,
					daIpv4Count: 3,
					saIpv6Count: 4,
					daIpv6Count: 5,
					structureJsonSa: '[{"q":2,"tauTilde":8,"sd":0.25}]',
					structureJsonDa: null,
					spectrumJsonSa: '[{"alpha":1.5,"f":2}]',
					spectrumJsonDa: null
				}
			])
		} as never);

		const response = await getDetails({
			params: { slug: '202503010005' },
			url: new URL('http://localhost/api/netflow/files/x/details?dataset=alpha')
		} as never);

		expect(response.status).toBe(200);
		await expect(response.json()).resolves.toEqual({
			routers: [
				{
					summary: {
						router: 'r1',
						file_path: '/captures/r1/nfcapd.202503010005',
						file_exists_on_disk: false,
						input_kind: 'nfcapd',
						input_status: 'processed',
						input_error_message: null,
						bucket_start: 1740823500,
						bucket_end: 1740823800,
						flows: 1,
						flows_tcp: 2,
						flows_udp: 3,
						flows_icmp: 4,
						flows_other: 5,
						packets: 6,
						packets_tcp: 7,
						packets_udp: 8,
						packets_icmp: 9,
						packets_other: 10,
						bytes: 11,
						bytes_tcp: 12,
						bytes_udp: 13,
						bytes_icmp: 14,
						bytes_other: 15,
						first_timestamp: 16,
						last_timestamp: 17,
						msec_first: 18,
						msec_last: 19,
						sequence_failures: 20,
						processed_at: 'now'
					},
					ipCountsSource: { ipv4Count: 2, ipv6Count: 4 },
					ipCountsDestination: { ipv4Count: 3, ipv6Count: 5 },
					structureSource: {
						slug: '202503010005',
						router: 'r1',
						filename: 'nfcapd.202503010005',
						structureFunction: [{ q: 2, tau: 8, sd: 0.25 }],
						metadata: {
							dataSource: 'Database: structure_stats 5m bucket (Source Addresses)',
							uniqueIPCount: -1,
							pointCount: 1,
							addressType: 'Source',
							qRange: { min: 2, max: 2 }
						}
					},
					structureDestination: null,
					spectrumSource: {
						slug: '202503010005',
						router: 'r1',
						filename: 'nfcapd.202503010005',
						spectrum: [{ alpha: 1.5, f: 2 }],
						metadata: {
							dataSource: 'Database: spectrum_stats 5m bucket (Source Addresses)',
							uniqueIPCount: -1,
							pointCount: 1,
							addressType: 'Source',
							alphaRange: { min: 1.5, max: 1.5 }
						}
					},
					spectrumDestination: null
				}
			]
		});
	});

	it('handles singularities validation and missing-file failures', async () => {
		const originalShowSingularities = process.env.SHOW_SINGULARITIES;

		try {
			process.env.SHOW_SINGULARITIES = 'true';
			const badResponse = await getSingularities({
				params: { slug: '202503010005' },
				url: new URL('http://localhost/api/netflow/files/x/singularities?router=r1&source=maybe')
			} as never);
			const missingFileResponse = await getSingularities({
				params: { slug: '202503010005' },
				url: new URL('http://localhost/api/netflow/files/x/singularities?router=r1&source=true')
			} as never);

			expect(badResponse.status).toBe(410);
			expect(missingFileResponse.status).toBe(410);
		} finally {
			if (originalShowSingularities === undefined) {
				delete process.env.SHOW_SINGULARITIES;
			} else {
				process.env.SHOW_SINGULARITIES = originalShowSingularities;
			}
		}
	});

	it('hides singularities route unless SHOW_SINGULARITIES is true', async () => {
		const originalShowSingularities = process.env.SHOW_SINGULARITIES;

		try {
			process.env.SHOW_SINGULARITIES = 'false';

			const response = await getSingularities({
				params: { slug: '202503010005' },
				url: new URL('http://localhost/api/netflow/files/x/singularities?router=r1&source=true')
			} as never);

			expect(response.status).toBe(410);
		} finally {
			if (originalShowSingularities === undefined) {
				delete process.env.SHOW_SINGULARITIES;
			} else {
				process.env.SHOW_SINGULARITIES = originalShowSingularities;
			}
		}
	});
});
