import type { GroupByOption } from '#lib/components/netflow/types.ts';
import { FLOW_VISIBILITIES, type FlowVisibility } from '#lib/types/types.ts';
import { z } from 'zod';

export type DateRangeSearch = {
	startDate: string;
	endDate: string;
	groupBy: GroupByOption;
	srcVisibility: FlowVisibility;
	dstVisibility: FlowVisibility;
};

type SearchParamsReader = Pick<URLSearchParams, 'get' | 'toString'>;

const GROUP_BY_OPTIONS = ['date', 'hour', '30min', '5min'] as const satisfies GroupByOption[];

export function createDateRangeSearch(
	defaultStartDate: string,
	today = new Date().toJSON().slice(0, 10)
) {
	const schema = z.object({
		startDate: z.iso.date().catch(defaultStartDate),
		endDate: z.iso.date().catch(today),
		groupBy: z.enum(GROUP_BY_OPTIONS).catch('date'),
		srcVisibility: z.enum(FLOW_VISIBILITIES).catch('all'),
		dstVisibility: z.enum(FLOW_VISIBILITIES).catch('all')
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
			if (raw === null ? next[key] === defaults[key] : raw === next[key]) continue;
			if (next[key] === defaults[key]) params.delete(key);
			else params.set(key, next[key]);
		}
		return params;
	}

	function equals(left: DateRangeSearch, right: DateRangeSearch): boolean {
		return keys.every((key) => left[key] === right[key]);
	}

	return { defaults, parse, serialize, equals };
}
