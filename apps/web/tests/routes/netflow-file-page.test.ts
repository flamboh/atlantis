import { afterEach, describe, expect, it, vi } from 'vitest';
import { load } from '../../src/routes/netflow/files/[slug]/+page.server';

describe('/netflow/files/[slug] page load', () => {
	afterEach(() => {
		vi.unstubAllEnvs();
	});

	it('returns page props with stored-stat singularities disabled', async () => {
		vi.stubEnv('SHOW_SINGULARITIES', 'true');

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
						sourceCount: 1,
						isDefault: true
					}
				],
				error: null
			})
		});

		const result = await load({
			params: { slug: '202503010005' },
			url: new URL(
				'http://localhost/netflow/files/202503010005?srcVisibility=literal&dstVisibility=anonymized'
			),
			fetch
		} as never);

		expect(fetch).toHaveBeenCalledWith('/api/datasets');
		expect(result).toEqual({
			dataset: 'alpha',
			slug: '202503010005',
			srcVisibility: 'literal',
			dstVisibility: 'anonymized',
			showSingularities: false,
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

	it('rejects invalid visibility params', async () => {
		await expect(
			load({
				params: { slug: '202503010005' },
				url: new URL('http://localhost/netflow/files/202503010005?dstVisibility=bogus'),
				fetch: vi.fn()
			} as never)
		).rejects.toMatchObject({
			status: 400,
			body: {
				message: 'Invalid dstVisibility. Expected one of: all, literal, anonymized'
			}
		});
	});
});
