import { describe, expect, it, vi } from 'vitest';

vi.mock('$app/paths', () => ({
	resolve: (_routeId: string, params: Record<string, string>) => `/netflow/files/${params.slug}`
}));

describe('buildNetflowFileHref', () => {
	it('builds only the dataset search when provided', async () => {
		const { buildNetflowFileSearch } = await import('$lib/utils/netflow-file-navigation');

		expect(buildNetflowFileSearch('uoregon')).toBe('?dataset=uoregon');
		expect(buildNetflowFileSearch(' uoregon ')).toBe('?dataset=uoregon');
		expect(buildNetflowFileSearch('my dataset')).toBe('?dataset=my%20dataset');
	});

	it('includes a non-all direction when provided', async () => {
		const { buildNetflowFileHref, buildNetflowFileSearch } =
			await import('$lib/utils/netflow-file-navigation');

		expect(buildNetflowFileSearch('uoregon', 'ingress')).toBe('?dataset=uoregon&direction=ingress');
		expect(buildNetflowFileHref('202506192010', 'uoregon', 'ingress')).toBe(
			'/netflow/files/202506192010?dataset=uoregon&direction=ingress'
		);
	});

	it('omits an all direction', async () => {
		const { buildNetflowFileSearch } = await import('$lib/utils/netflow-file-navigation');

		expect(buildNetflowFileSearch('uoregon', 'all')).toBe('?dataset=uoregon');
	});

	it('includes dataset query when provided', async () => {
		const { buildNetflowFileHref } = await import('$lib/utils/netflow-file-navigation');

		expect(buildNetflowFileHref('202506192010', 'uoregon')).toBe(
			'/netflow/files/202506192010?dataset=uoregon'
		);
		expect(buildNetflowFileHref('202506192010', ' uoregon ')).toBe(
			'/netflow/files/202506192010?dataset=uoregon'
		);
		expect(buildNetflowFileHref('202506192010', 'my dataset')).toBe(
			'/netflow/files/202506192010?dataset=my%20dataset'
		);
	});

	it('omits query when dataset is empty', async () => {
		const { buildNetflowFileHref, buildNetflowFileSearch } =
			await import('$lib/utils/netflow-file-navigation');

		expect(buildNetflowFileHref('202506192010', '')).toBe('/netflow/files/202506192010');
		expect(buildNetflowFileSearch('')).toBe('');
		expect(buildNetflowFileSearch('   ')).toBe('');
	});

	it('adds a non-default MAAD IP family to file links', async () => {
		const { buildNetflowFileHref, buildNetflowFileSearch } =
			await import('$lib/utils/netflow-file-navigation');

		expect(buildNetflowFileSearch('uoregon', 'all', 4)).toBe('?dataset=uoregon');
		expect(buildNetflowFileSearch('uoregon', 'ingress', 6)).toBe(
			'?dataset=uoregon&direction=ingress&ipVersion=6'
		);
		expect(buildNetflowFileHref('202506192010', 'uoregon', 'all', 6)).toBe(
			'/netflow/files/202506192010?dataset=uoregon&ipVersion=6'
		);
	});

	it('delegates navigation to the built href', async () => {
		const { navigateToNetflowFile } = await import('$lib/utils/netflow-file-navigation');
		const navigate = vi.fn().mockResolvedValue(undefined);

		await navigateToNetflowFile(navigate, '202506192010', 'uoregon');

		expect(navigate).toHaveBeenCalledWith('/netflow/files/202506192010?dataset=uoregon', undefined);

		await navigateToNetflowFile(navigate, '202506192010', 'my dataset', 'egress');

		expect(navigate).toHaveBeenLastCalledWith(
			'/netflow/files/202506192010?dataset=my%20dataset&direction=egress',
			undefined
		);

		await navigateToNetflowFile(navigate, '202506192010', 'uoregon', 'all', 6, {
			replaceState: true
		});

		expect(navigate).toHaveBeenLastCalledWith(
			'/netflow/files/202506192010?dataset=uoregon&ipVersion=6',
			{ replaceState: true }
		);
	});
});
