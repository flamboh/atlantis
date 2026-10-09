const ISO_DATE = /^\d{4}-\d{2}-\d{2}$/;
const DAY_MS = 86_400_000;

export function isIsoDate(value: string): boolean {
	if (!ISO_DATE.test(value)) return false;
	const date = utc(value);
	return !Number.isNaN(date.getTime()) && date.toISOString().slice(0, 10) === value;
}

function addDays(date: string, days: number): string {
	return new Date(utc(date).getTime() + days * DAY_MS).toISOString().slice(0, 10);
}

const dayFormat = new Intl.DateTimeFormat('en-US', {
	month: 'short',
	day: 'numeric',
	timeZone: 'UTC'
});
const dayYearFormat = new Intl.DateTimeFormat('en-US', {
	month: 'short',
	day: 'numeric',
	year: 'numeric',
	timeZone: 'UTC'
});

function utc(date: string): Date {
	return new Date(`${date}T00:00:00Z`);
}

/** Format an inclusive YYYY-MM-DD range compactly, e.g. "Mar 1 – Mar 3, 2025". */
export function formatDateRange(startDate: string, endDate: string): string {
	if (!isIsoDate(startDate) || !isIsoDate(endDate)) return `${startDate} – ${endDate}`;
	if (startDate === endDate) return dayYearFormat.format(utc(startDate));
	const sameYear = startDate.slice(0, 4) === endDate.slice(0, 4);
	return `${(sameYear ? dayFormat : dayYearFormat).format(utc(startDate))} – ${dayYearFormat.format(utc(endDate))}`;
}

export type DateRangePreset = { id: string; label: string; startDate: string; endDate: string };

/** Window presets end at the selected end date; "All available" spans the dataset start to today. */
export function dateRangePresets(
	endDate: string,
	datasetStartDate: string,
	today: string
): DateRangePreset[] {
	const anchor = isIsoDate(endDate) ? endDate : today;
	const windows = [
		{ id: '1d', label: 'Last day', days: 1 },
		{ id: '7d', label: 'Last 7 days', days: 7 },
		{ id: '30d', label: 'Last 30 days', days: 30 }
	];
	return [
		...windows.map(({ id, label, days }) => ({
			id,
			label,
			startDate: addDays(anchor, 1 - days),
			endDate: anchor
		})),
		{ id: 'all', label: 'All available', startDate: datasetStartDate, endDate: today }
	];
}
