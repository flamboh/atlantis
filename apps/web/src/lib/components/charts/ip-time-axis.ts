import type { IpGranularity } from '$lib/types/types';
import { epochToPSTComponents, getWeekdayName } from '$lib/utils/timezone';

export function formatTemporalBucketLabel(bucketStart: number, granularity: IpGranularity): string {
	const pst = epochToPSTComponents(bucketStart);
	const year = pst.year;
	const month = `${pst.month}`.padStart(2, '0');
	const day = `${pst.day}`.padStart(2, '0');
	const hours = `${pst.hours}`.padStart(2, '0');
	const minutes = `${pst.minutes}`.padStart(2, '0');

	if (granularity === '1d') {
		return `${year}-${month}-${day}`;
	}

	return `${year}-${month}-${day} ${hours}:${minutes}`;
}

export function formatIpGranularityTick(
	bucketStart: number,
	granularity: IpGranularity,
	index: number
): string {
	const pst = epochToPSTComponents(bucketStart);
	const day = pst.day.toString().padStart(2, '0');
	const month = pst.month.toString().padStart(2, '0');
	const hours = pst.hours;
	const minutes = pst.minutes;
	const weekday = getWeekdayName(pst.dayOfWeek);

	if (granularity === '1d') {
		return pst.dayOfWeek === 1 ? `Mon ${month}/${day}` : '';
	}

	if (granularity === '1h') {
		return hours === 0 ? `${weekday} ${pst.month}/${pst.day}` : '';
	}

	if (granularity === '30m') {
		if (minutes === 0 && (hours === 0 || hours === 12)) {
			return `${weekday} ${pst.month}/${pst.day} ${hours.toString().padStart(2, '0')}:00`;
		}
		return '';
	}

	if (granularity === '5m') {
		return minutes === 0
			? `${weekday} ${pst.month}/${pst.day} ${hours.toString().padStart(2, '0')}:00`
			: '';
	}

	return index === 0 ? `${weekday} ${pst.month}/${pst.day}` : '';
}

export function shouldHighlightIpGranularityGrid(
	bucketStart: number,
	granularity: IpGranularity,
	index: number
): boolean {
	const pst = epochToPSTComponents(bucketStart);
	const hours = pst.hours;
	const minutes = pst.minutes;

	if (granularity === '1d') {
		return pst.dayOfWeek === 1;
	}
	if (granularity === '1h') {
		return hours === 0;
	}
	if (granularity === '30m') {
		return minutes === 0 && (hours === 0 || hours === 12);
	}
	if (granularity === '5m') {
		return minutes === 0;
	}
	return index === 0;
}
