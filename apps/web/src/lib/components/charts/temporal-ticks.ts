export function temporalTicks(
	values: readonly number[],
	format: (value: number) => string,
	compact = false
): number[] {
	const labeled = values.filter((value) => Number.isFinite(value) && format(value));
	if (!compact || labeled.length <= 4) return labeled;
	return Array.from(
		{ length: 4 },
		(_, index) => labeled[Math.round((index * (labeled.length - 1)) / 3)]
	);
}
