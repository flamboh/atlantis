import { describe, expect, it } from 'vitest';
import { createDateRangeSearch } from '#lib/schemas.ts';

const search = createDateRangeSearch('2025-01-01', '2025-02-01');

describe('createDateRangeSearch', () => {
	it('falls back to defaults for missing params', () => {
		expect(search.parse(new URLSearchParams())).toEqual({
			startDate: '2025-01-01',
			endDate: '2025-02-01',
			groupBy: 'date',
			direction: 'all',
			ipVersion: 4,
			measure: 'addresses'
		});
	});

	it('parses valid params and falls back per key for invalid ones', () => {
		expect(
			search.parse(
				new URLSearchParams(
					'startDate=2025-01-10&endDate=nope&groupBy=hour&direction=bogus&ipVersion=6&measure=bytes'
				)
			)
		).toEqual({
			startDate: '2025-01-10',
			endDate: '2025-02-01',
			groupBy: 'hour',
			direction: 'all',
			ipVersion: 6,
			measure: 'bytes'
		});
	});

	it('preserves Gregorian date validation, including leap years and four-digit years', () => {
		for (const date of [
			'2025-02-29',
			'2024-02-30',
			'2025-04-31',
			'2025-13-01',
			'2025-00-01',
			'2025-01-00',
			'2025-1-01'
		]) {
			expect(search.parse(new URLSearchParams({ startDate: date })).startDate).toBe('2025-01-01');
		}
		for (const date of ['2024-02-29', '2000-02-29', '0000-01-01', '9999-12-31']) {
			expect(search.parse(new URLSearchParams({ startDate: date })).startDate).toBe(date);
		}
	});

	it('accepts every dashboard grouping including 10 minutes', () => {
		for (const groupBy of ['date', 'hour', '30min', '10min', '5min']) {
			expect(search.parse(new URLSearchParams({ groupBy })).groupBy).toBe(groupBy);
		}
	});

	it('rejects stored granularity spellings as groupings', () => {
		expect(search.parse(new URLSearchParams('groupBy=10m')).groupBy).toBe('date');
	});

	it('accepts every flow direction', () => {
		for (const direction of ['all', 'ingress', 'egress', 'lateral', 'transit']) {
			expect(search.parse(new URLSearchParams({ direction })).direction).toBe(direction);
		}
	});

	it('falls back to IPv4 for unknown MAAD IP families', () => {
		for (const ipVersion of ['5', '6.0', 'v6', '']) {
			expect(search.parse(new URLSearchParams({ ipVersion })).ipVersion).toBe(4);
		}
	});

	it('accepts weighted MAAD measures and falls back for unknown ones', () => {
		for (const measure of ['addresses', 'packets', 'bytes']) {
			expect(search.parse(new URLSearchParams({ measure })).measure).toBe(measure);
		}
		expect(search.parse(new URLSearchParams('measure=flows')).measure).toBe('addresses');
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

	it('writes MAAD params and removes them when they return to defaults', () => {
		const params = search.serialize(new URLSearchParams(), {
			...search.defaults,
			direction: 'ingress',
			ipVersion: 6,
			measure: 'packets'
		});
		expect(params.toString()).toBe('direction=ingress&ipVersion=6&measure=packets');
		expect(search.serialize(params, search.defaults).toString()).toBe('');
	});

	it('removes params that change back to their defaults', () => {
		const current = new URLSearchParams('startDate=2025-01-10&groupBy=hour');
		const params = search.serialize(current, { ...search.defaults, startDate: '2025-01-10' });
		expect(params.toString()).toBe('startDate=2025-01-10');
	});

	it('leaves unchanged explicit params in place', () => {
		const current = new URLSearchParams('groupBy=date&endDate=2025-01-20&ipVersion=4');
		const params = search.serialize(current, { ...search.defaults, endDate: '2025-01-20' });
		expect(params.toString()).toBe('groupBy=date&endDate=2025-01-20&ipVersion=4');
	});

	it('drops invalid params when the parsed value is written back', () => {
		const current = new URLSearchParams('groupBy=bogus&startDate=2025-01-10&ipVersion=5');
		const params = search.serialize(current, search.parse(current));
		expect(params.toString()).toBe('startDate=2025-01-10');
	});

	it('compares parsed values key by key', () => {
		expect(search.equals(search.defaults, { ...search.defaults })).toBe(true);
		expect(search.equals(search.defaults, { ...search.defaults, groupBy: 'hour' })).toBe(false);
		expect(search.equals(search.defaults, { ...search.defaults, ipVersion: 6 })).toBe(false);
	});
});
