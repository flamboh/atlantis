import {
	IP_METRIC_OPTIONS,
	type IpGranularity,
	type IpMetricKey,
	type IpStatsBucket,
	type ProtocolMetricKey,
	type ProtocolStatsBucket
} from '#lib/types/types.ts';
import type { DimensionMetricKey, DimensionStatsPayload } from '#lib/types/dimension-stats.ts';

export type BreakdownChartKind = 'ip' | 'protocol' | 'dimensions' | 'spectrum';
export type BreakdownMetricKey = IpMetricKey | ProtocolMetricKey | DimensionMetricKey;
export type LineBucketData = IpStatsBucket | ProtocolStatsBucket | DimensionStatsPayload;

export interface LineMetricConfig {
	key: BreakdownMetricKey;
	label: string;
	seriesLabel: string;
	color?: string;
}

export interface BreakdownChartConfig {
	kind: BreakdownChartKind;
	chartId: BreakdownChartKind;
	title: string;
	endpoint: string;
	usesMaad: boolean;
	seriesByRouter: boolean;
	defaultGranularity: IpGranularity;
	defaultMetrics: BreakdownMetricKey[];
	metrics: LineMetricConfig[];
	fillAlpha: number;
	yAxisTitle: string;
	formatYAxisTicks: boolean;
	fitYAxisToData: boolean;
	loadingCopy: string;
	emptyCopy: string;
	noMetricsCopy: string;
	noSourceCopy: string;
	fetchErrorCopy: string;
	unexpectedErrorCopy: string;
	chartLabel: string;
}

const ipMetricLabels: Record<IpMetricKey, string> = {
	saIpv4Count: 'Src IPv4',
	daIpv4Count: 'Dst IPv4',
	saIpv6Count: 'Src IPv6',
	daIpv6Count: 'Dst IPv6'
};

const ipColors: Record<IpMetricKey, string> = {
	saIpv4Count: 'var(--chart-series-1)',
	daIpv4Count: 'var(--chart-series-2)',
	saIpv6Count: 'var(--chart-series-3)',
	daIpv6Count: 'var(--chart-series-4)'
};

const IP_CONFIG: BreakdownChartConfig = {
	kind: 'ip',
	chartId: 'ip',
	title: 'Unique IP Counts',
	endpoint: '/api/ip/stats',
	usesMaad: false,
	seriesByRouter: false,
	defaultGranularity: '1d',
	defaultMetrics: ['saIpv4Count', 'daIpv4Count'],
	metrics: IP_METRIC_OPTIONS.map((option) => ({
		key: option.key,
		label: option.label,
		seriesLabel: ipMetricLabels[option.key],
		color: ipColors[option.key]
	})),
	fillAlpha: 0.18,
	yAxisTitle: 'Unique IPs',
	formatYAxisTicks: true,
	fitYAxisToData: false,
	loadingCopy: 'Loading IP data...',
	emptyCopy: 'No IP data for the selected window.',
	noMetricsCopy: 'Select at least one metric to display.',
	noSourceCopy: 'Select at least one source to view IP statistics',
	fetchErrorCopy: 'Failed to load IP statistics',
	unexpectedErrorCopy: 'Unexpected error loading IP statistics',
	chartLabel: 'IP chart'
};

const PROTOCOL_CONFIG: BreakdownChartConfig = {
	kind: 'protocol',
	chartId: 'protocol',
	title: 'Unique Protocol Counts',
	endpoint: '/api/protocol/stats',
	usesMaad: false,
	seriesByRouter: false,
	defaultGranularity: '1h',
	defaultMetrics: ['uniqueProtocolsIpv4', 'uniqueProtocolsIpv6'],
	metrics: [
		{
			key: 'uniqueProtocolsIpv4',
			label: 'Unique Protocols IPv4',
			seriesLabel: 'IPv4',
			color: 'var(--chart-series-1)'
		},
		{
			key: 'uniqueProtocolsIpv6',
			label: 'Unique Protocols IPv6',
			seriesLabel: 'IPv6',
			color: 'var(--chart-series-2)'
		}
	],
	fillAlpha: 0.2,
	yAxisTitle: 'Unique Protocols',
	formatYAxisTicks: false,
	fitYAxisToData: false,
	loadingCopy: 'Loading protocol data...',
	emptyCopy: 'No protocol data for the selected window.',
	noMetricsCopy: 'Select at least one metric to display.',
	noSourceCopy: 'Select at least one source to view protocol statistics',
	fetchErrorCopy: 'Failed to load protocol statistics',
	unexpectedErrorCopy: 'Unexpected error loading protocol statistics',
	chartLabel: 'Protocol chart'
};

export type DimensionSide = 'sa' | 'da';
export type DimensionOrder = '0' | '1' | '2';

export const DIMENSION_SIDE_OPTIONS: Array<{ value: DimensionSide; label: string }> = [
	{ value: 'sa', label: 'Source' },
	{ value: 'da', label: 'Destination' }
];

export const DIMENSION_ORDER_OPTIONS: Array<{
	value: DimensionOrder;
	label: string;
	title: string;
}> = [
	{ value: '0', label: 'D0', title: 'Capacity dimension' },
	{ value: '1', label: 'D1', title: 'Information dimension' },
	{ value: '2', label: 'D2', title: 'Correlation dimension' }
];

export function dimensionMetricKey(side: DimensionSide, order: DimensionOrder): DimensionMetricKey {
	return `${side}D${order}`;
}

export function splitDimensionMetricKey(key: DimensionMetricKey): {
	side: DimensionSide;
	order: DimensionOrder;
} {
	return { side: key.slice(0, 2) as DimensionSide, order: key.slice(3) as DimensionOrder };
}

const DIMENSIONS_CONFIG: BreakdownChartConfig = {
	kind: 'dimensions',
	chartId: 'dimensions',
	title: 'MAAD Dimensions',
	endpoint: '/api/netflow/dimension-stats',
	usesMaad: true,
	seriesByRouter: true,
	defaultGranularity: '1h',
	defaultMetrics: ['saD1'],
	metrics: DIMENSION_SIDE_OPTIONS.flatMap((side) =>
		DIMENSION_ORDER_OPTIONS.map((order) => {
			const label = `${side.label} ${order.label}`;
			return {
				key: dimensionMetricKey(side.value, order.value),
				label,
				seriesLabel: label
			};
		})
	),
	fillAlpha: 0,
	yAxisTitle: 'Dimension',
	formatYAxisTicks: false,
	fitYAxisToData: true,
	loadingCopy: 'Loading MAAD dimensions...',
	emptyCopy: 'No MAAD dimensions for the selected window.',
	noMetricsCopy: 'Select at least one dimension to display.',
	noSourceCopy: 'Select at least one source to view MAAD dimensions',
	fetchErrorCopy: 'Failed to load MAAD dimensions',
	unexpectedErrorCopy: 'Unexpected error loading MAAD dimensions',
	chartLabel: 'MAAD dimensions chart'
};

const SPECTRUM_CONFIG: BreakdownChartConfig = {
	kind: 'spectrum',
	chartId: 'spectrum',
	title: 'Spectrum',
	endpoint: '/api/netflow/spectrum-stats',
	usesMaad: true,
	seriesByRouter: false,
	defaultGranularity: '1h',
	defaultMetrics: [],
	metrics: [],
	fillAlpha: 0,
	yAxisTitle: 'alpha',
	formatYAxisTicks: false,
	fitYAxisToData: false,
	loadingCopy: 'Loading spectrum data...',
	emptyCopy: 'No spectrum data for the selected window.',
	noMetricsCopy: '',
	noSourceCopy: 'Select at least one source to view spectrum statistics',
	fetchErrorCopy: 'Failed to load spectrum statistics',
	unexpectedErrorCopy: 'Unexpected error loading spectrum statistics',
	chartLabel: 'Spectrum chart'
};

export const BREAKDOWN_CHART_CONFIGS: Record<BreakdownChartKind, BreakdownChartConfig> = {
	ip: IP_CONFIG,
	protocol: PROTOCOL_CONFIG,
	dimensions: DIMENSIONS_CONFIG,
	spectrum: SPECTRUM_CONFIG
};

export function readLineMetric(
	data: LineBucketData | null,
	key: BreakdownMetricKey
): number | null {
	if (!data || !(key in data)) {
		return null;
	}
	const value = data[key as keyof LineBucketData];
	return typeof value === 'number' ? value : null;
}
