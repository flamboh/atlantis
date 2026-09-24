import type { goto } from '$app/navigation';

export function navigateToSearchParams(
	navigate: typeof goto,
	params: URLSearchParams
): Promise<void> {
	return navigate(`?${params}`, { reset: false });
}
