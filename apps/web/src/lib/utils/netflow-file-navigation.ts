import type { goto } from '$app/navigation';
import { resolve } from '$app/paths';
import { DEFAULT_MAAD_IP_VERSION, type FlowDirection, type MaadIpVersion } from '$lib/types/types';

export function buildNetflowFileSearch(
	dataset?: string,
	direction?: FlowDirection,
	ipVersion?: MaadIpVersion
): string {
	const normalizedDataset = dataset?.trim();
	const searchParams: string[] = [];

	if (normalizedDataset) {
		searchParams.push(`dataset=${encodeURIComponent(normalizedDataset)}`);
	}

	if (direction && direction !== 'all') {
		searchParams.push(`direction=${encodeURIComponent(direction)}`);
	}

	if (ipVersion && ipVersion !== DEFAULT_MAAD_IP_VERSION) {
		searchParams.push(`ipVersion=${ipVersion}`);
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
	direction?: FlowDirection,
	ipVersion?: MaadIpVersion
): string {
	const pathname = resolve('/netflow/files/[slug]', { slug });
	return `${pathname}${buildNetflowFileSearch(dataset, direction, ipVersion)}`;
}

export function navigateToNetflowFile(
	navigate: typeof goto,
	slug: string,
	dataset?: string,
	direction?: FlowDirection,
	ipVersion?: MaadIpVersion,
	options?: Parameters<typeof goto>[1]
): Promise<void> {
	return navigate(buildNetflowFileHref(slug, dataset, direction, ipVersion), options);
}
