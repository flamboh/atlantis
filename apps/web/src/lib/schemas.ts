import type { GroupByOption } from '#lib/components/netflow/types.ts';
import {
	DEFAULT_MAAD_IP_VERSION,
	DEFAULT_MAAD_MEASURE,
	FLOW_DIRECTIONS,
	MAAD_IP_VERSIONS,
	MAAD_MEASURES,
	type FlowDirection,
	type MaadIpVersion,
	type MaadMeasure
} from '#lib/types/types.ts';
import { z } from 'zod';

export type DateRangeSearch = {
	startDate: string;
	endDate: string;
	groupBy: GroupByOption;
	direction: FlowDirection;
	ipVersion: MaadIpVersion;
	measure: MaadMeasure;
};

type SearchParamsReader = Pick<URLSearchParams, 'get' | 'toString'>;

const GROUP_BY_OPTIONS = [
	'date',
	'hour',
	'30min',
	'10min',
	'5min'
] as const satisfies GroupByOption[];

const IP_VERSION_PARAMS = MAAD_IP_VERSIONS.map(String) as [string, ...string[]];

export function createDateRangeSearch(
	defaultStartDate: string,
	today = new Date().toJSON().slice(0, 10)
) {
	const schema = z.object({
		startDate: z.iso.date().catch(defaultStartDate),
		endDate: z.iso.date().catch(today),
		groupBy: z.enum(GROUP_BY_OPTIONS).catch('date'),
		direction: z.enum(FLOW_DIRECTIONS).catch('all'),
		ipVersion: z
			.enum(IP_VERSION_PARAMS)
			.transform((value) => Number(value) as MaadIpVersion)
			.catch(DEFAULT_MAAD_IP_VERSION),
		measure: z.enum(MAAD_MEASURES).catch(DEFAULT_MAAD_MEASURE)
	});
	const keys = schema.keyof().options;
	const defaults: DateRangeSearch = schema.parse({});

	function parse(searchParams: SearchParamsReader): DateRangeSearch {
		return schema.parse(Object.fromEntries(keys.map((key) => [key, searchParams.get(key)])));
	}

	function serialize(current: SearchParamsReader, next: DateRangeSearch): URLSearchParams {
		const params = new URLSearchParams(current.toString());
		for (const key of keys) {
			const raw = current.get(key);
			const value = String(next[key]);
			if (raw === null ? next[key] === defaults[key] : raw === value) continue;
			if (next[key] === defaults[key]) params.delete(key);
			else params.set(key, value);
		}
		return params;
	}

	function equals(left: DateRangeSearch, right: DateRangeSearch): boolean {
		return keys.every((key) => left[key] === right[key]);
	}

	return { defaults, parse, serialize, equals };
}
