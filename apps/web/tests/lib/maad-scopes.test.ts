import { describe, expect, it } from 'vitest';
import {
	FLOW_DIRECTIONS,
	MAAD_ADDRESS_SIDES,
	maadInternalSideCopy,
	maadSideComputed
} from '../../src/lib/types/types';

describe('internal-side MAAD scopes', () => {
	it('marks only internal address sides as skipped', () => {
		const skipped = Object.fromEntries(
			FLOW_DIRECTIONS.map((direction) => [
				direction,
				MAAD_ADDRESS_SIDES.filter((side) => !maadSideComputed(direction, side, false))
			])
		);

		expect(skipped).toEqual({
			all: [],
			ingress: ['destination'],
			egress: ['source'],
			lateral: ['source', 'destination'],
			transit: []
		});
	});

	it('computes every side when the product opted in', () => {
		for (const direction of FLOW_DIRECTIONS) {
			for (const side of MAAD_ADDRESS_SIDES) {
				expect(maadSideComputed(direction, side, true)).toBe(true);
			}
		}
	});

	it('names the skipped sides and direction', () => {
		expect(maadInternalSideCopy('ingress', ['destination'])).toBe(
			'MAAD is not computed for internal addresses. The destination addresses are internal for ingress traffic.'
		);
		expect(maadInternalSideCopy('lateral', ['source', 'destination'])).toBe(
			'MAAD is not computed for internal addresses. The source and destination addresses are both internal for lateral traffic.'
		);
	});
});
