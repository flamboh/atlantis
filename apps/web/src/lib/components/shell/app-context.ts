import { resolve } from '$app/paths';
import type { DatasetSummary } from '#lib/datasets.ts';

export type AppSection = 'datasets' | 'dashboard' | 'files' | 'alerts';

type PageLike = {
	params: Partial<Record<string, string>>;
	url: { searchParams: Pick<URLSearchParams, 'get'> };
	data: Record<string, unknown>;
};

export function pageDatasetId(page: PageLike): string | null {
	const fromData = page.data.selectedDataset ?? page.data.dataset ?? page.data.datasetId;
	return (
		page.params.dataset ??
		(page.url.searchParams.get('dataset')?.trim() || null) ??
		(typeof fromData === 'string' && fromData ? fromData : null)
	);
}

export function pageDatasets(page: PageLike): DatasetSummary[] | null {
	return Array.isArray(page.data.datasets) ? (page.data.datasets as DatasetSummary[]) : null;
}

export function pageSection(pathname: string): AppSection {
	if (pathname.startsWith('/netflow/files')) return 'files';
	if (/^\/datasets\/[^/]+\/alerts/.test(pathname)) return 'alerts';
	if (pathname.startsWith('/datasets/')) return 'dashboard';
	return 'datasets';
}

export function sectionHref(section: AppSection, datasetId: string | null): string {
	if (section === 'files') {
		return datasetId
			? `${resolve('/netflow/files')}?dataset=${encodeURIComponent(datasetId)}`
			: resolve('/netflow/files');
	}
	if (!datasetId || section === 'datasets') return resolve('/');
	if (section === 'alerts') return resolve('/datasets/[dataset]/alerts', { dataset: datasetId });
	return resolve('/datasets/[dataset]', { dataset: datasetId });
}

export function sectionsFor(datasetId: string | null): { section: AppSection; label: string }[] {
	return datasetId
		? [
				{ section: 'dashboard', label: 'Dashboard' },
				{ section: 'files', label: 'Files' },
				{ section: 'alerts', label: 'Alerts' }
			]
		: [
				{ section: 'datasets', label: 'Datasets' },
				{ section: 'files', label: 'Files' }
			];
}
