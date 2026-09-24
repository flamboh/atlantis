import { describe, expect, it } from 'vitest';
import { createDateRangeSearch } from '#lib/schemas.ts';

const search = createDateRangeSearch('2025-01-01', '2025-02-01');

describe('createDateRangeSearch', () => {
	it('falls back to defaults for missing params', () => {
		expect(search.parse(new URLSearchParams())).toEqual({
			startDate: '2025-01-01',
			endDate: '2025-02-01',
			groupBy: 'date',
			srcVisibility: 'all',
			dstVisibility: 'all'
		});
	});

	it('parses valid params and falls back per key for invalid ones', () => {
		expect(
			search.parse(
				new URLSearchParams(
					'startDate=2025-01-10&endDate=nope&groupBy=hour&srcVisibility=bogus&dstVisibility=literal'
				)
			)
		).toEqual({
			startDate: '2025-01-10',
			endDate: '2025-02-01',
			groupBy: 'hour',
			srcVisibility: 'all',
			dstVisibility: 'literal'
		});
	});

	it('omits default values and keeps unrelated params', () => {
		const current = new URLSearchParams('foo=bar');
		const params = search.serialize(current, {
			...search.defaults,
			startDate: '2025-01-10',
			groupBy: 'hour'
		});
		expect(params.toString()).toBe('foo=bar&startDate=2025-01-10&groupBy=hour');
	});

	it('removes params that change back to their defaults', () => {
		const current = new URLSearchParams('startDate=2025-01-10&groupBy=hour');
		const params = search.serialize(current, { ...search.defaults, startDate: '2025-01-10' });
		expect(params.toString()).toBe('startDate=2025-01-10');
	});

	it('leaves unchanged explicit params in place', () => {
		const current = new URLSearchParams('groupBy=date&endDate=2025-01-20');
		const params = search.serialize(current, { ...search.defaults, endDate: '2025-01-20' });
		expect(params.toString()).toBe('groupBy=date&endDate=2025-01-20');
	});

	it('drops invalid params when the parsed value is written back', () => {
		const current = new URLSearchParams('groupBy=bogus&startDate=2025-01-10');
		const params = search.serialize(current, search.parse(current));
		expect(params.toString()).toBe('startDate=2025-01-10');
	});

	it('compares parsed values key by key', () => {
		expect(search.equals(search.defaults, { ...search.defaults })).toBe(true);
		expect(search.equals(search.defaults, { ...search.defaults, groupBy: 'hour' })).toBe(false);
	});
});
