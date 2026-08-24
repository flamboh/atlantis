import { describe, expect, it } from 'vitest';
import {
	dateStringToEpochPST,
	epochToPSTComponents,
	parseLabelToPSTComponents
} from '../../src/lib/utils/timezone';

describe('Pacific timezone conversion', () => {
	it('preserves wall-clock components and weekdays across repeated conversions', () => {
		const epoch = dateStringToEpochPST('2026-03-02');

		expect(epochToPSTComponents(epoch)).toMatchObject({
			year: 2026,
			month: 3,
			day: 2,
			hours: 0,
			minutes: 0,
			dayOfWeek: 1
		});
		expect(parseLabelToPSTComponents('2026-03-02 13:30')?.dayOfWeek).toBe(1);
		expect(epochToPSTComponents(epoch)).toEqual(epochToPSTComponents(epoch));
	});

	it('does not let callers mutate memoized components', () => {
		const epoch = dateStringToEpochPST('2026-03-02');
		const components = epochToPSTComponents(epoch);

		expect(Reflect.set(components, 'year', 1900)).toBe(false);
		expect(epochToPSTComponents(epoch).year).toBe(2026);
	});

	it('keeps date ranges aligned across the daylight-saving transition', () => {
		const start = dateStringToEpochPST('2026-03-08');
		const end = dateStringToEpochPST('2026-03-08', true);

		expect(end - start).toBe(23 * 60 * 60);
		expect(epochToPSTComponents(end)).toMatchObject({
			year: 2026,
			month: 3,
			day: 9,
			hours: 0
		});
	});
});
