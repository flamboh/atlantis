import type { goto } from '$app/navigation';
import { resolve } from '$app/paths';
import {
	DEFAULT_MAAD_IP_VERSION,
	DEFAULT_MAAD_MEASURE,
	type FlowDirection,
	type MaadIpVersion,
	type MaadMeasure
} from '#lib/types/types.ts';

export function buildNetflowFileSearch(
	dataset?: string,
	direction?: FlowDirection,
	ipVersion?: MaadIpVersion,
	measure?: MaadMeasure
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

	if (measure && measure !== DEFAULT_MAAD_MEASURE) {
		searchParams.push(`measure=${encodeURIComponent(measure)}`);
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
	ipVersion?: MaadIpVersion,
	measure?: MaadMeasure
): string {
	const pathname = resolve('/netflow/files/[slug]', { slug });
	return `${pathname}${buildNetflowFileSearch(dataset, direction, ipVersion, measure)}`;
}

export function navigateToNetflowFile(
	navigate: typeof goto,
	slug: string,
	dataset?: string,
	direction?: FlowDirection,
	ipVersion?: MaadIpVersion,
	measure?: MaadMeasure,
	options?: Parameters<typeof goto>[1]
): Promise<void> {
	return navigate(buildNetflowFileHref(slug, dataset, direction, ipVersion, measure), options);
}
