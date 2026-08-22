import type { NetflowMetricTotals, TimeBucket } from '$lib/types/types';

export type NetflowDataPoint = TimeBucket<NetflowMetricTotals>;

export interface DataOption {
	label: string;
	index: number;
	checked: boolean;
}

export interface RouterConfig {
	[key: string]: boolean;
}

export type GroupByOption = 'date' | 'hour' | '30min' | '5min';

export type ChartTypeOption = 'stacked' | 'line';

export interface ChartState {
	startDate: string;
	endDate: string;
	routers: RouterConfig;
	groupBy: GroupByOption;
	chartType: ChartTypeOption;
	dataOptions: DataOption[];
}

export interface ClickedElement {
	dataset: {
		label: string;
		data: (number | null)[];
		backgroundColor?: string;
		borderColor?: string;
	};
	label: string;
	value: number;
	datasetIndex: number;
	index: number;
}

export interface ChartDataset {
	label: string;
	data: (number | null)[];
	backgroundColor?: string | string[];
	borderColor?: string | string[];
	borderWidth?: number;
	fill?: boolean | string;
	tension?: number;
	pointRadius?: number | number[];
	pointBackgroundColor?: string | string[];
	pointBorderColor?: string | string[];
	pointBorderWidth?: number | number[];
	spanGaps?: boolean;
	segment?: Record<string, unknown>;
	pointHoverRadius?: number;
	radius?: number;
	hitRadius?: number;
	hoverRadius?: number;
}

export interface ChartConfig {
	type: 'line' | 'bar' | 'stacked';
	data: {
		labels: string[];
		datasets: ChartDataset[];
	};
	options: {
		onClick?: (
			event: MouseEvent,
			activeElements: { datasetIndex: number; index: number }[]
		) => void;
		responsive: boolean;
		animation?: boolean;
		maintainAspectRatio?: boolean;
		scales?: Record<string, object>;
		plugins?: Record<string, object>;
		interaction?: Record<string, unknown>;
	};
}
