type CoverageState = {
	state: 'complete' | 'partial' | 'unknown';
};

export type CoveragePointStyle = {
	pointRadius: number[];
	pointBackgroundColor: string[];
	pointBorderColor: string;
	pointBorderWidth: number[];
};

const PARTIAL_POINT_RADIUS = 3;
const PARTIAL_POINT_BORDER_WIDTH = 2;
const TRANSPARENT_POINT_FILL = 'rgba(0, 0, 0, 0)';

/** Build array styles only when a series has a numeric partial observation to mark. */
export function buildCoveragePointStyle<Point>(
	points: readonly Point[],
	getValue: (point: Point, index: number) => number | null,
	getCoverage: (point: Point, index: number) => CoverageState,
	color: string
): CoveragePointStyle | null {
	let firstPartialPoint = -1;
	for (let index = 0; index < points.length; index += 1) {
		const point = points[index];
		if (point === undefined) continue;
		const value = getValue(point, index);
		if (getCoverage(point, index).state === 'partial' && Number.isFinite(value)) {
			firstPartialPoint = index;
			break;
		}
	}
	if (firstPartialPoint === -1) return null;

	const pointRadius = new Array<number>(points.length).fill(0);
	const pointBackgroundColor = new Array<string>(points.length).fill(color);
	const pointBorderWidth = new Array<number>(points.length).fill(0);
	pointRadius[firstPartialPoint] = PARTIAL_POINT_RADIUS;
	pointBackgroundColor[firstPartialPoint] = TRANSPARENT_POINT_FILL;
	pointBorderWidth[firstPartialPoint] = PARTIAL_POINT_BORDER_WIDTH;

	for (let index = firstPartialPoint + 1; index < points.length; index += 1) {
		const point = points[index];
		if (point === undefined) continue;
		const value = getValue(point, index);
		if (getCoverage(point, index).state !== 'partial' || !Number.isFinite(value)) continue;

		pointRadius[index] = PARTIAL_POINT_RADIUS;
		pointBackgroundColor[index] = TRANSPARENT_POINT_FILL;
		pointBorderWidth[index] = PARTIAL_POINT_BORDER_WIDTH;
	}

	return { pointRadius, pointBackgroundColor, pointBorderColor: color, pointBorderWidth };
}
