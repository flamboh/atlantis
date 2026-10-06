export function coverageLineRuns<Point>(
	points: readonly Point[],
	getValue: (point: Point) => number | null,
	isPartial: (point: Point) => boolean
): Array<{ points: Point[]; partial: boolean }> {
	const runs: Array<{ points: Point[]; partial: boolean }> = [];
	let previous: Point | undefined;
	let current: { points: Point[]; partial: boolean } | undefined;
	for (const point of points) {
		const value = getValue(point);
		if (value === null || !Number.isFinite(value)) {
			previous = undefined;
			current = undefined;
			continue;
		}
		if (!previous) {
			current = { points: [point], partial: isPartial(point) };
			runs.push(current);
		} else {
			const partial = isPartial(previous) || isPartial(point);
			if (current && (current.partial === partial || current.points.length === 1)) {
				current.partial = partial;
				current.points.push(point);
			} else {
				current = { points: [previous, point], partial };
				runs.push(current);
			}
		}
		previous = point;
	}
	return runs;
}
