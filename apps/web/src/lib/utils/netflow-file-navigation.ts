import type { goto } from '$app/navigation';
import { resolve } from '$app/paths';
import type { FlowDirection } from '$lib/types/types';

export function buildNetflowFileSearch(dataset?: string, direction?: FlowDirection): string {
	const normalizedDataset = dataset?.trim();
	const searchParams: string[] = [];

	if (normalizedDataset) {
		searchParams.push(`dataset=${encodeURIComponent(normalizedDataset)}`);
	}

	if (direction && direction !== 'all') {
		searchParams.push(`direction=${encodeURIComponent(direction)}`);
	}

	const search = searchParams.join('&');
	if (!search) {
		return '';
	}

	return `?${search}`;
}

export function buildNetflowFileHref(
	slug: string,
	dataset?: string,
	direction?: FlowDirection
): string {
	const pathname = resolve('/netflow/files/[slug]', { slug });
	return `${pathname}${buildNetflowFileSearch(dataset, direction)}`;
}

export function navigateToNetflowFile(
	navigate: typeof goto,
	slug: string,
	dataset?: string,
	direction?: FlowDirection
): Promise<void> {
	return navigate(buildNetflowFileHref(slug, dataset, direction));
}
