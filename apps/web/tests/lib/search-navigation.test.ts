import { describe, expect, it, vi } from 'vitest';
import type { goto } from '$app/navigation';
import { navigateToSearchParams } from '#lib/utils/search-navigation.ts';

describe('navigateToSearchParams', () => {
	it('pushes a new history entry without resetting scroll or focus', async () => {
		const navigate = vi.fn<typeof goto>().mockResolvedValue();
		await navigateToSearchParams(
			navigate,
			new URLSearchParams('startDate=2025-01-02&groupBy=hour')
		);
		expect(navigate).toHaveBeenCalledWith('?startDate=2025-01-02&groupBy=hour', { reset: false });
	});
});
