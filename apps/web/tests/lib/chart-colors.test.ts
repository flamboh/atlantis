import { describe, expect, it } from 'vitest';
import { spectrumInterpolator } from '../../src/lib/components/charts/chart-colors';

describe('spectrum colors', () => {
	it('preserves the purple, blue, cyan, green, yellow order and clamps outliers', () => {
		const color = spectrumInterpolator();
		expect([0, 1 / 7, 3 / 7, 5 / 7, 1].map(color)).toEqual([
			'hsl(270, 70%, 50%)',
			'hsl(240, 70%, 50%)',
			'hsl(180, 70%, 50%)',
			'hsl(120, 70%, 50%)',
			'hsl(60, 70%, 50%)'
		]);
		expect(color(-1)).toBe(color(0));
		expect(color(2)).toBe(color(1));
	});
	it('uses theme saturation and lightness without reducing opacity', () => {
		expect(
			spectrumInterpolator({ lowHue: 270, highHue: 60, saturation: 70, lightness: 60 })(0.5)
		).toBe('hsl(165, 70%, 60%)');
	});
});
