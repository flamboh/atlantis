import {
	IP_METRIC_OPTIONS,
	type IpGranularity,
	type IpMetricKey,
	type IpStatsBucket,
	type ProtocolMetricKey,
	type ProtocolStatsBucket
} from '$lib/types/types';

export type BreakdownChartKind = 'ip' | 'protocol' | 'spectrum';
export type BreakdownMetricKey = IpMetricKey | ProtocolMetricKey;
export type LineBucketData = IpStatsBucket | ProtocolStatsBucket;

export interface LineMetricConfig {
	key: BreakdownMetricKey;
	label: string;
	seriesLabel: string;
	color: {
		hue: number;
		saturation: number;
		lightness: number;
	};
}

export interface BreakdownChartConfig {
	kind: BreakdownChartKind;
	chartId: BreakdownChartKind;
	title: string;
	endpoint: string;
	defaultGranularity: IpGranularity;
	defaultMetrics: BreakdownMetricKey[];
	metrics: LineMetricConfig[];
	routerHueStep: number;
	fillAlpha: number;
	yAxisTitle: string;
	formatYAxisTicks: boolean;
	loadingCopy: string;
	emptyCopy: string;
	noMetricsCopy: string;
	noSourceCopy: string;
	fetchErrorCopy: string;
	unexpectedErrorCopy: string;
	canvasLabel: string;
}

const ipMetricLabels: Record<IpMetricKey, string> = {
	saIpv4Count: 'Src IPv4',
	daIpv4Count: 'Dst IPv4',
	saIpv6Count: 'Src IPv6',
	daIpv6Count: 'Dst IPv6'
};

const ipColors: Record<IpMetricKey, { hue: number; saturation: number; lightness: number }> = {
	saIpv4Count: { hue: 130, saturation: 65, lightness: 58 },
	daIpv4Count: { hue: 130, saturation: 65, lightness: 38 },
	saIpv6Count: { hue: 25, saturation: 72, lightness: 60 },
	daIpv6Count: { hue: 25, saturation: 72, lightness: 40 }
};

const IP_CONFIG: BreakdownChartConfig = {
	kind: 'ip',
	chartId: 'ip',
	title: 'Unique IP Counts',
	endpoint: '/api/ip/stats',
	defaultGranularity: '1d',
	defaultMetrics: ['saIpv4Count', 'daIpv4Count'],
	metrics: IP_METRIC_OPTIONS.map((option) => ({
		key: option.key,
		label: option.label,
		seriesLabel: ipMetricLabels[option.key],
		color: ipColors[option.key]
	})),
	routerHueStep: 70,
	fillAlpha: 0.18,
	yAxisTitle: 'Unique IPs',
	formatYAxisTicks: true,
	loadingCopy: 'Loading IP data...',
	emptyCopy: 'No IP data for the selected window.',
	noMetricsCopy: 'Select at least one metric to display.',
	noSourceCopy: 'Select at least one source to view IP statistics',
	fetchErrorCopy: 'Failed to load IP statistics',
	unexpectedErrorCopy: 'Unexpected error loading IP statistics',
	canvasLabel: 'IP chart'
};

const PROTOCOL_CONFIG: BreakdownChartConfig = {
	kind: 'protocol',
	chartId: 'protocol',
	title: 'Unique Protocol Counts',
	endpoint: '/api/protocol/stats',
	defaultGranularity: '1h',
	defaultMetrics: ['uniqueProtocolsIpv4', 'uniqueProtocolsIpv6'],
	metrics: [
		{
			key: 'uniqueProtocolsIpv4',
			label: 'Unique Protocols IPv4',
			seriesLabel: 'IPv4',
			color: { hue: 210, saturation: 70, lightness: 50 }
		},
		{
			key: 'uniqueProtocolsIpv6',
			label: 'Unique Protocols IPv6',
			seriesLabel: 'IPv6',
			color: { hue: 35, saturation: 75, lightness: 48 }
		}
	],
	routerHueStep: 110,
	fillAlpha: 0.2,
	yAxisTitle: 'Unique Protocols',
	formatYAxisTicks: false,
	loadingCopy: 'Loading protocol data...',
	emptyCopy: 'No protocol data for the selected window.',
	noMetricsCopy: 'Select at least one metric to display.',
	noSourceCopy: 'Select at least one source to view protocol statistics',
	fetchErrorCopy: 'Failed to load protocol statistics',
	unexpectedErrorCopy: 'Unexpected error loading protocol statistics',
	canvasLabel: 'Protocol chart'
};

const SPECTRUM_CONFIG: BreakdownChartConfig = {
	kind: 'spectrum',
	chartId: 'spectrum',
	title: 'Spectrum',
	endpoint: '/api/netflow/spectrum-stats',
	defaultGranularity: '1h',
	defaultMetrics: [],
	metrics: [],
	routerHueStep: 0,
	fillAlpha: 0,
	yAxisTitle: 'alpha',
	formatYAxisTicks: false,
	loadingCopy: 'Loading spectrum data...',
	emptyCopy: 'No spectrum data for the selected window.',
	noMetricsCopy: '',
	noSourceCopy: 'Select at least one source to view spectrum statistics',
	fetchErrorCopy: 'Failed to load spectrum statistics',
	unexpectedErrorCopy: 'Unexpected error loading spectrum statistics',
	canvasLabel: 'Spectrum chart'
};

export const BREAKDOWN_CHART_CONFIGS: Record<BreakdownChartKind, BreakdownChartConfig> = {
	ip: IP_CONFIG,
	protocol: PROTOCOL_CONFIG,
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
