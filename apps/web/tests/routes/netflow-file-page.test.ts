import { describe, expect, it, vi } from 'vitest';
import { load } from '../../src/routes/netflow/files/[slug]/+page.server';

describe('/netflow/files/[slug] page load', () => {
	it('returns page props for stored file stats', async () => {
		const fetch = vi.fn().mockResolvedValue({
			ok: true,
			status: 200,
			json: async () => ({
				data: [
					{
						datasetId: 'alpha',
						label: 'Alpha',
						defaultStartDate: '2025-02-11',
						discoveryMode: 'db',
						hasLocality: false,
						isDefault: true
					}
				],
				error: null
			})
		});

		const result = await load({
			params: { slug: '202503010005' },
			url: new URL('http://localhost/netflow/files/202503010005?direction=ingress'),
			fetch
		} as never);

		expect(fetch).toHaveBeenCalledWith('/api/datasets');
		expect(result).toEqual({
			dataset: 'alpha',
			slug: '202503010005',
			direction: 'ingress',
			ipVersion: 4,
			measure: 'addresses',
			fileInfo: {
				year: '2025',
				month: '03',
				day: '01',
				hour: '00',
				minute: '05',
				filename: 'nfcapd.202503010005'
			}
		});
	});

	it('reads the MAAD IP family from the search params', async () => {
		const result = await load({
			params: { slug: '202503010005' },
			url: new URL('http://localhost/netflow/files/202503010005?dataset=alpha&ipVersion=6'),
			fetch: vi.fn()
		} as never);

		expect(result).toMatchObject({ dataset: 'alpha', direction: 'all', ipVersion: 6 });
	});

	it('rejects invalid ipVersion params', async () => {
		await expect(
			load({
				params: { slug: '202503010005' },
				url: new URL('http://localhost/netflow/files/202503010005?dataset=alpha&ipVersion=5'),
				fetch: vi.fn()
			} as never)
		).rejects.toMatchObject({
			status: 400,
			body: { message: 'Invalid ipVersion. Expected one of: 4, 6' }
		});
	});

	it('rejects invalid direction params', async () => {
		await expect(
			load({
				params: { slug: '202503010005' },
				url: new URL('http://localhost/netflow/files/202503010005?direction=bogus'),
				fetch: vi.fn()
			} as never)
		).rejects.toMatchObject({
			status: 400,
			body: {
				message: 'Invalid direction. Expected one of: all, ingress, egress, lateral, transit'
			}
		});
	});

	it('reads the MAAD measure from the URL and rejects unknown measures', async () => {
		const result = await load({
			params: { slug: '202503010005' },
			url: new URL('http://localhost/netflow/files/202503010005?dataset=alpha&measure=bytes'),
			fetch: vi.fn()
		} as never);
		expect(result).toMatchObject({ dataset: 'alpha', direction: 'all', measure: 'bytes' });

		await expect(
			load({
				params: { slug: '202503010005' },
				url: new URL('http://localhost/netflow/files/202503010005?dataset=alpha&measure=flows'),
				fetch: vi.fn()
			} as never)
		).rejects.toMatchObject({
			status: 400,
			body: { message: 'Invalid measure. Expected one of: addresses, packets, bytes' }
		});
	});
});
