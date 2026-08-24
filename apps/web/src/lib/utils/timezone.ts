/**
 * Timezone utilities for consistent PST/PDT timestamp handling.
 *
 * All netflow data represents events at the University of Oregon (America/Los_Angeles timezone).
 * These utilities ensure timestamps are displayed consistently regardless of the viewer's timezone.
 */

const PST_TIMEZONE = 'America/Los_Angeles';
const WEEKDAY_BY_NAME: Readonly<Record<string, number>> = {
	Sun: 0,
	Mon: 1,
	Tue: 2,
	Wed: 3,
	Thu: 4,
	Fri: 5,
	Sat: 6
};
const PST_COMPONENT_FORMATTER = new Intl.DateTimeFormat('en-US', {
	timeZone: PST_TIMEZONE,
	year: 'numeric',
	month: '2-digit',
	day: '2-digit',
	hour: '2-digit',
	minute: '2-digit',
	second: '2-digit',
	weekday: 'short',
	hour12: false
});
const PST_WALL_CLOCK_FORMATTER = new Intl.DateTimeFormat('en-US', {
	timeZone: PST_TIMEZONE,
	year: 'numeric',
	month: '2-digit',
	day: '2-digit',
	hour: '2-digit',
	minute: '2-digit',
	second: '2-digit',
	hour12: false
});
const PST_DISPLAY_FORMATTER = new Intl.DateTimeFormat('en-US', {
	timeZone: PST_TIMEZONE,
	year: 'numeric',
	month: '2-digit',
	day: '2-digit',
	hour: '2-digit',
	minute: '2-digit',
	second: '2-digit',
	hour12: true
});
const EPOCH_COMPONENT_CACHE_LIMIT = 8_192;
const epochComponentCache = new Map<number, PSTDateComponents>();

/**
 * Parse a date string (YYYY-MM-DD or YYYY-MM-DD HH:mm) as PST and return
 * an object with the date components in PST, without timezone conversion.
 *
 * This returns the "wall clock" values as they would appear in PST.
 */
export interface PSTDateComponents {
	readonly year: number;
	readonly month: number; // 1-12
	readonly day: number;
	readonly hours: number;
	readonly minutes: number;
	readonly seconds: number;
	readonly dayOfWeek: number; // 0=Sunday, 6=Saturday
}

/**
 * Parse a label string like "2024-01-15" or "2024-01-15 08:00" and extract
 * the date components as PST wall-clock time.
 *
 * Returns null if the string cannot be parsed.
 */
export function parseLabelToPSTComponents(label: string): PSTDateComponents | null {
	if (!label) return null;

	// Match YYYY-MM-DD or YYYY-MM-DD HH:mm
	const match = label.match(/^(\d{4})-(\d{2})-(\d{2})(?:\s+(\d{2}):(\d{2}))?$/);
	if (!match) return null;

	const year = parseInt(match[1], 10);
	const month = parseInt(match[2], 10);
	const day = parseInt(match[3], 10);
	const hours = match[4] ? parseInt(match[4], 10) : 0;
	const minutes = match[5] ? parseInt(match[5], 10) : 0;

	if (!Number.isFinite(year) || !Number.isFinite(month) || !Number.isFinite(day)) {
		return null;
	}

	// Once the Pacific wall-clock date is parsed, its Gregorian weekday is timezone-independent.
	const dayOfWeek = new Date(Date.UTC(year, month - 1, day)).getUTCDay();

	return {
		year,
		month,
		day,
		hours,
		minutes,
		seconds: 0,
		dayOfWeek
	};
}

/**
 * Create a JavaScript Date object that represents a specific PST wall-clock time.
 * The returned Date will have a UTC timestamp that corresponds to the given PST time.
 */
export function createDateFromPSTComponents(
	year: number,
	month: number, // 1-12
	day: number,
	hours: number = 0,
	minutes: number = 0,
	seconds: number = 0
): Date {
	// We need to find the UTC timestamp that corresponds to this PST time
	// Use an iterative approach to account for DST transitions
	// Start with a guess (PST is typically UTC-8 or UTC-7 during DST)
	let guess = new Date(Date.UTC(year, month - 1, day, hours + 8, minutes, seconds));

	// Adjust based on actual PST time at that moment
	for (let i = 0; i < 3; i++) {
		const parts = PST_WALL_CLOCK_FORMATTER.formatToParts(guess);
		const getPart = (type: string) =>
			parseInt(parts.find((p) => p.type === type)?.value ?? '0', 10);

		const actualHour = getPart('hour');
		const actualMinute = getPart('minute');
		const actualDay = getPart('day');
		const actualMonth = getPart('month');

		const hourDiff = hours - actualHour;
		const minuteDiff = minutes - actualMinute;
		const dayDiff = day - actualDay;
		const monthDiff = month - actualMonth;

		if (hourDiff === 0 && minuteDiff === 0 && dayDiff === 0 && monthDiff === 0) {
			break;
		}

		// Adjust by the difference
		guess = new Date(
			guess.getTime() +
				dayDiff * 24 * 60 * 60 * 1000 +
				hourDiff * 60 * 60 * 1000 +
				minuteDiff * 60 * 1000
		);
	}

	return guess;
}

/**
 * Convert a date string (YYYY-MM-DD) to epoch seconds, treating the date as PST.
 * If isEnd is true, returns the end of the day (start of next day).
 */
export function dateStringToEpochPST(dateString: string, isEnd: boolean = false): number {
	const match = dateString.match(/^(\d{4})-(\d{2})-(\d{2})$/);
	if (!match) return 0;

	const year = parseInt(match[1], 10);
	const month = parseInt(match[2], 10);
	const day = parseInt(match[3], 10);

	if (!Number.isFinite(year) || !Number.isFinite(month) || !Number.isFinite(day)) {
		return 0;
	}

	let date: Date;
	if (isEnd) {
		// End of day = start of next day at 00:00:00 PST
		// Add one day
		const nextDay = new Date(Date.UTC(year, month - 1, day + 1));
		date = createDateFromPSTComponents(
			nextDay.getUTCFullYear(),
			nextDay.getUTCMonth() + 1,
			nextDay.getUTCDate(),
			0,
			0,
			0
		);
	} else {
		date = createDateFromPSTComponents(year, month, day, 0, 0, 0);
	}

	return Math.floor(date.getTime() / 1000);
}

/**
 * Format an epoch timestamp (seconds) as PST date components.
 */
export function epochToPSTComponents(epochSeconds: number): PSTDateComponents {
	const cached = epochComponentCache.get(epochSeconds);
	if (cached) return cached;

	const date = new Date(epochSeconds * 1000);
	const parts = PST_COMPONENT_FORMATTER.formatToParts(date);
	const getPart = (type: string) => parts.find((p) => p.type === type)?.value ?? '';

	const weekdayStr = getPart('weekday');
	const components: PSTDateComponents = Object.freeze({
		year: parseInt(getPart('year'), 10),
		month: parseInt(getPart('month'), 10),
		day: parseInt(getPart('day'), 10),
		hours: parseInt(getPart('hour'), 10),
		minutes: parseInt(getPart('minute'), 10),
		seconds: parseInt(getPart('second'), 10),
		dayOfWeek: WEEKDAY_BY_NAME[weekdayStr] ?? 0
	});
	if (epochComponentCache.size >= EPOCH_COMPONENT_CACHE_LIMIT) {
		const oldestEpoch = epochComponentCache.keys().next().value;
		if (oldestEpoch !== undefined) epochComponentCache.delete(oldestEpoch);
	}
	epochComponentCache.set(epochSeconds, components);
	return components;
}

/**
 * Format a PST date components object as a label string.
 */
export function formatPSTLabel(components: PSTDateComponents, includeTime: boolean = true): string {
	const datePart = `${components.year}-${String(components.month).padStart(2, '0')}-${String(components.day).padStart(2, '0')}`;

	if (!includeTime) {
		return datePart;
	}

	return `${datePart} ${String(components.hours).padStart(2, '0')}:${String(components.minutes).padStart(2, '0')}`;
}

/**
 * Get the weekday name for a day of week number.
 */
export function getWeekdayName(dayOfWeek: number): string {
	return ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'][dayOfWeek] ?? '';
}

/**
 * Create a Date object for PST drill-down calculations.
 * Returns a Date that can be used for time arithmetic (adding days, etc.)
 */
export function parseLabelToDateForDrilldown(label: string): Date | null {
	const components = parseLabelToPSTComponents(label);
	if (!components) return null;

	return createDateFromPSTComponents(
		components.year,
		components.month,
		components.day,
		components.hours,
		components.minutes
	);
}

/**
 * Format a Date (that represents a PST moment) back to a YYYY-MM-DD string,
 * preserving the PST date (not converting to local timezone).
 */
export function formatDateAsPSTDateString(date: Date): string {
	const components = epochToPSTComponents(Math.floor(date.getTime() / 1000));
	return `${components.year}-${String(components.month).padStart(2, '0')}-${String(components.day).padStart(2, '0')}`;
}

/**
 * Format an epoch timestamp (seconds or milliseconds) as a human-readable PST string.
 * Similar to toLocaleString() but always shows PST.
 */
export function formatTimestampAsPST(epochMs: number): string {
	// Handle both seconds and milliseconds
	const ms = epochMs < 10000000000 ? epochMs * 1000 : epochMs;
	const date = new Date(ms);

	return `${PST_DISPLAY_FORMATTER.format(date)} PST`;
}
