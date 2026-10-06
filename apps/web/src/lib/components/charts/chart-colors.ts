export type SpectrumColors = {
	lowHue: number;
	highHue: number;
	saturation: number;
	lightness: number;
};

export function spectrumInterpolator(
	colors: SpectrumColors = {
		lowHue: 270,
		highHue: 60,
		saturation: 70,
		lightness: 50
	}
) {
	return (position: number) => {
		const value = Math.max(0, Math.min(1, position));
		return `hsl(${colors.lowHue + value * (colors.highHue - colors.lowHue)}, ${colors.saturation}%, ${colors.lightness}%)`;
	};
}

export function readSpectrumColors(style: CSSStyleDeclaration, dark: boolean): SpectrumColors {
	return {
		lowHue: Number(style.getPropertyValue('--chart-spectrum-low-hue') || 270),
		highHue: Number(style.getPropertyValue('--chart-spectrum-high-hue') || 60),
		saturation: Number(style.getPropertyValue('--chart-spectrum-saturation') || 70),
		lightness: Number(style.getPropertyValue('--chart-spectrum-lightness') || (dark ? 60 : 50))
	};
}

export function sourceSeriesColor(color: string, sourceIndex: number, hueStep = 80): string {
	return sourceIndex === 0
		? color
		: `hsl(from ${color} calc(h + ${(sourceIndex * hueStep) % 360}) s l)`;
}

export function categoricalSeriesColor(index: number): string {
	return sourceSeriesColor(`var(--chart-series-${(index % 8) + 1})`, Math.floor(index / 8), 20);
}
