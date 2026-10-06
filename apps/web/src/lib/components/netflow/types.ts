import type { NetflowMetricTotals, TimeBucket } from '#lib/types/types.ts';

export type NetflowDataPoint = TimeBucket<NetflowMetricTotals>;

export interface DataOption {
	label: string;
	index: number;
	checked: boolean;
}

export interface RouterConfig {
	[key: string]: boolean;
}

export type GroupByOption = 'date' | 'hour' | '30min' | '10min' | '5min';

export type ChartTypeOption = 'stacked' | 'line';

export interface ChartState {
	startDate: string;
	endDate: string;
	routers: RouterConfig;
	groupBy: GroupByOption;
	chartType: ChartTypeOption;
	dataOptions: DataOption[];
}
