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

	it('includes non-all visibility scope when provided', async () => {
		const { buildNetflowFileHref, buildNetflowFileSearch } =
			await import('$lib/utils/netflow-file-navigation');
		const flowScope = { srcVisibility: 'literal', dstVisibility: 'anonymized' } as const;

		expect(buildNetflowFileSearch('uoregon', flowScope)).toBe(
			'?dataset=uoregon&srcVisibility=literal&dstVisibility=anonymized'
		);
		expect(buildNetflowFileHref('202506192010', 'uoregon', flowScope)).toBe(
			'/netflow/files/202506192010?dataset=uoregon&srcVisibility=literal&dstVisibility=anonymized'
		);
	});

	it('omits all/all visibility scope', async () => {
		const { buildNetflowFileSearch } = await import('$lib/utils/netflow-file-navigation');

		expect(buildNetflowFileSearch('uoregon', { srcVisibility: 'all', dstVisibility: 'all' })).toBe(
			'?dataset=uoregon'
		);
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

	it('delegates navigation to the built href', async () => {
		const { navigateToNetflowFile } = await import('$lib/utils/netflow-file-navigation');
		const navigate = vi.fn().mockResolvedValue(undefined);

		await navigateToNetflowFile(navigate, '202506192010', 'uoregon');

		expect(navigate).toHaveBeenCalledWith('/netflow/files/202506192010?dataset=uoregon');

		await navigateToNetflowFile(navigate, '202506192010', 'my dataset', {
			srcVisibility: 'anonymized',
			dstVisibility: 'literal'
		});

		expect(navigate).toHaveBeenLastCalledWith(
			'/netflow/files/202506192010?dataset=my%20dataset&srcVisibility=anonymized&dstVisibility=literal'
		);
	});
});
