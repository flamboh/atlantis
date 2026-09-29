import type { SpectrumPoint } from '#lib/types/types.ts';

export function finiteSpectrumPoints(points: SpectrumPoint[]): SpectrumPoint[] {
	return points.filter((point) => Number.isFinite(point.alpha) && Number.isFinite(point.f));
}

export function paddedSpectrumBounds(min: number, max: number) {
	const padding = Math.max((max - min) * 0.05, Math.max(Math.abs(min), Math.abs(max), 1) * 0.01);
	return { min: min - padding, max: max + padding };
}
