import type { GroupByOption } from '#lib/components/netflow/types.ts';
import { formatDateAsPSTDateString } from '#lib/utils/timezone.ts';
import {
	chooseAdaptiveGranularity,
	generateSlugFromLabel,
	groupByBucketDurationMs,
	parseClickedLabel
} from './chart-utils';
import type { PlotPoint } from './chart-registry';

export type Drilldown = (groupBy: GroupByOption, startDate: string, endDate: string) => void;

export function openTemporalPoint(
	point: PlotPoint,
	groupBy: GroupByOption,
	onDrillDown?: Drilldown,
	onFile?: (slug: string) => void
) {
	if (!point.label) return;
	if (groupBy === '5min') {
		const slug = generateSlugFromLabel(point.label, groupBy);
		if (slug) onFile?.(slug);
		return;
	}
	const clicked = parseClickedLabel(point.label, groupBy);
	if (!Number.isFinite(clicked.getTime())) return;
	const day = 86400000;
	const start =
		groupBy === 'date'
			? new Date(clicked.getTime() - 15 * day)
			: groupBy === 'hour'
				? new Date(clicked.getTime() - 3 * day)
				: clicked;
	const end = new Date(
		clicked.getTime() + (groupBy === 'date' ? 16 : groupBy === 'hour' ? 4 : 1) * day
	);
	onDrillDown?.(
		groupBy === 'date' ? 'hour' : groupBy === 'hour' ? '10min' : '5min',
		formatDateAsPSTDateString(start),
		formatDateAsPSTDateString(end)
	);
}

export function openTemporalRange(
	start: PlotPoint,
	end: PlotPoint,
	groupBy: GroupByOption,
	onDrillDown?: Drilldown
) {
	if (!start.label || !end.label) return;
	const from = parseClickedLabel(start.label, groupBy);
	const to = new Date(
		parseClickedLabel(end.label, groupBy).getTime() + groupByBucketDurationMs(groupBy)
	);
	if (!Number.isFinite(from.getTime()) || !Number.isFinite(to.getTime())) return;
	onDrillDown?.(
		chooseAdaptiveGranularity(to.getTime() - from.getTime()),
		formatDateAsPSTDateString(from),
		formatDateAsPSTDateString(to)
	);
}
