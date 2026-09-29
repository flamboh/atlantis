import { describe, expect, it } from 'vitest';
import {
	finiteSpectrumPoints,
	paddedSpectrumBounds
} from '../../src/lib/components/charts/spectrum-points';

describe('spectrum display points', () => {
	it('keeps finite zero and negative coordinates while dropping non-finite pairs', () => {
		expect(
			finiteSpectrumPoints([
				{ alpha: 0, f: 0 },
				{ alpha: -1, f: -2 },
				{ alpha: Number.NaN, f: 1 },
				{ alpha: 1, f: Infinity }
			])
		).toEqual([
			{ alpha: 0, f: 0 },
			{ alpha: -1, f: -2 }
		]);
	});

	it('gives constant and nearly constant spectra a visible axis range', () => {
		expect(paddedSpectrumBounds(0, 0)).toEqual({ min: -0.01, max: 0.01 });
		expect(paddedSpectrumBounds(0.2, 0.2).min).toBeCloseTo(0.19);
		expect(paddedSpectrumBounds(0.2, 0.2).max).toBeCloseTo(0.21);
		const bounds = paddedSpectrumBounds(0.2, 0.20000001);
		expect(bounds.max - bounds.min).toBeGreaterThan(0.02);
		expect(paddedSpectrumBounds(0, 2)).toEqual({ min: -0.1, max: 2.1 });
	});
});
