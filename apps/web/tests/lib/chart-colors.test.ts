import { describe, expect, it } from 'vitest';
import {
	spectrumInterpolator,
	sourceSeriesColor
} from '../../src/lib/components/charts/chart-colors';

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

describe('source series colors', () => {
	it('preserves the base metric token and separates router hues', () => {
		const color = 'var(--chart-series-1)';
		expect(sourceSeriesColor(color, 0)).toBe(color);
		expect(sourceSeriesColor(color, 1)).toBe('hsl(from var(--chart-series-1) calc(h + 80) s l)');
		expect(sourceSeriesColor(color, 2)).toBe('hsl(from var(--chart-series-1) calc(h + 160) s l)');
	});
});
