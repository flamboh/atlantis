import type { goto } from '$app/navigation';
import { resolve } from '$app/paths';
import type { FlowScope } from '$lib/types/types';

export function buildNetflowFileSearch(dataset?: string, flowScope?: Partial<FlowScope>): string {
	const normalizedDataset = dataset?.trim();
	const searchParams: string[] = [];

	if (normalizedDataset) {
		searchParams.push(`dataset=${encodeURIComponent(normalizedDataset)}`);
	}

	if (
		flowScope?.srcVisibility &&
		flowScope?.dstVisibility &&
		(flowScope.srcVisibility !== 'all' || flowScope.dstVisibility !== 'all')
	) {
		searchParams.push(`srcVisibility=${encodeURIComponent(flowScope.srcVisibility)}`);
		searchParams.push(`dstVisibility=${encodeURIComponent(flowScope.dstVisibility)}`);
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
	flowScope?: Partial<FlowScope>
): string {
	const pathname = resolve('/netflow/files/[slug]', { slug });
	return `${pathname}${buildNetflowFileSearch(dataset, flowScope)}`;
}

export function navigateToNetflowFile(
	navigate: typeof goto,
	slug: string,
	dataset?: string,
	flowScope?: Partial<FlowScope>
): Promise<void> {
	return navigate(buildNetflowFileHref(slug, dataset, flowScope));
}
