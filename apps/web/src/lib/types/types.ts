export interface DatasetSummary {
	datasetId: string;
	label: string;
	defaultStartDate: string;
	discoveryMode: string;
	sourceCount: number;
	isDefault: boolean;
}

export interface DatasetSummariesResponse {
	data: DatasetSummary[] | null;
	error: string | null;
}

export interface NetflowStatsRow {
	date: string;
	flows?: number;
	flows_tcp?: number;
	flows_udp?: number;
	flows_icmp?: number;
	flows_other?: number;
	packets?: number;
	packets_tcp?: number;
	packets_udp?: number;
	packets_icmp?: number;
	packets_other?: number;
	bytes?: number;
	bytes_tcp?: number;
	bytes_udp?: number;
	bytes_icmp?: number;
	bytes_other?: number;
}

export type NetflowMetricField =
	| 'flows'
	| 'flowsTcp'
	| 'flowsUdp'
	| 'flowsIcmp'
	| 'flowsOther'
	| 'packets'
	| 'packetsTcp'
	| 'packetsUdp'
	| 'packetsIcmp'
	| 'packetsOther'
	| 'bytes'
	| 'bytesTcp'
	| 'bytesUdp'
	| 'bytesIcmp'
	| 'bytesOther';

export type NetflowIpFamily = 'all' | 'ipv4' | 'ipv6';
export type NetflowSplitMetricField = `${NetflowMetricField}Ipv4` | `${NetflowMetricField}Ipv6`;

export interface NetflowMetricTotals {
	flows: number;
	flowsTcp: number;
	flowsUdp: number;
	flowsIcmp: number;
	flowsOther: number;
	packets: number;
	packetsTcp: number;
	packetsUdp: number;
	packetsIcmp: number;
	packetsOther: number;
	bytes: number;
	bytesTcp: number;
	bytesUdp: number;
	bytesIcmp: number;
	bytesOther: number;
}

export type NetflowMetricTotalsByIpFamily = {
	[key in NetflowSplitMetricField]: number;
};

export interface NetflowStatsResult extends NetflowMetricTotals, NetflowMetricTotalsByIpFamily {
	bucketStart: number;
	averageDurationMs: number | null;
	averageMinTtl: number | null;
	averageMaxTtl: number | null;
}

export interface NetflowStatsResponse {
	result: NetflowStatsResult[];
	availableIpFamilies: NetflowIpFamily[];
}

export type PortSide = 'source' | 'destination';
export type PortRange = 'low' | 'high';

export interface ObservationStatsBucket {
	bucketStart: number;
	bucketEnd: number;
	ipFamily: NetflowIpFamily;
	averageDurationMs: number | null;
	averageMinTtl: number | null;
	averageMaxTtl: number | null;
}

export interface PortCardinalityBucket {
	sourceId: string;
	bucketStart: number;
	bucketEnd: number;
	ipFamily: Exclude<NetflowIpFamily, 'all'>;
	portSide: PortSide;
	portRange: PortRange;
	uniquePortCount: number;
}

export interface FlowCharacteristicsResponse {
	observationBuckets: ObservationStatsBucket[];
	portBuckets: PortCardinalityBucket[];
	resolvedSources: string[];
}

export interface NetflowFileSummaryRecord {
	router: string;
	file_path: string | null;
	file_exists_on_disk?: boolean;
	input_kind?: string | null;
	input_status?: string | null;
	input_error_message?: string | null;
	bucket_start?: number;
	bucket_end?: number;
	flows: number;
	flows_tcp: number;
	flows_udp: number;
	flows_icmp: number;
	flows_other: number;
	packets: number;
	packets_tcp: number;
	packets_udp: number;
	packets_icmp: number;
	packets_other: number;
	bytes: number;
	bytes_tcp: number;
	bytes_udp: number;
	bytes_icmp: number;
	bytes_other: number;
	first_timestamp: number | null;
	last_timestamp: number | null;
	msec_first: number | null;
	msec_last: number | null;
	sequence_failures: number | null;
	processed_at: string | null;
}

export interface NetflowFileSummaryResponse {
	summary: NetflowFileSummaryRecord[];
}

export interface FileIpCounts {
	ipv4Count: number | null;
	ipv6Count: number | null;
}

export interface NetflowFileDetailsRouter {
	summary: NetflowFileSummaryRecord;
	ipCountsSource: FileIpCounts | null;
	ipCountsDestination: FileIpCounts | null;
	structureSource: StructureFunctionData | null;
	structureDestination: StructureFunctionData | null;
	spectrumSource: SpectrumData | null;
	spectrumDestination: SpectrumData | null;
}

export interface NetflowFileDetailsResponse {
	routers: NetflowFileDetailsRouter[];
}

export const IP_GRANULARITIES = ['5m', '30m', '1h', '1d'] as const;

export type IpGranularity = (typeof IP_GRANULARITIES)[number];
export const FLOW_VISIBILITIES = ['all', 'literal', 'anonymized'] as const;
export type FlowVisibility = (typeof FLOW_VISIBILITIES)[number];

export interface FlowScope {
	srcVisibility: FlowVisibility;
	dstVisibility: FlowVisibility;
}

export type FlowScopeKey =
	| 'all'
	| 'literal_to_literal'
	| 'literal_to_anonymized'
	| 'anonymized_to_literal'
	| 'anonymized_to_anonymized';

export interface FlowScopeOption extends FlowScope {
	key: FlowScopeKey;
	label: string;
}

export const FLOW_SCOPE_OPTIONS: FlowScopeOption[] = [
	{ key: 'all', label: 'all', srcVisibility: 'all', dstVisibility: 'all' },
	{
		key: 'literal_to_literal',
		label: 'literal to literal',
		srcVisibility: 'literal',
		dstVisibility: 'literal'
	},
	{
		key: 'literal_to_anonymized',
		label: 'literal to anonymized',
		srcVisibility: 'literal',
		dstVisibility: 'anonymized'
	},
	{
		key: 'anonymized_to_literal',
		label: 'anonymized to literal',
		srcVisibility: 'anonymized',
		dstVisibility: 'literal'
	},
	{
		key: 'anonymized_to_anonymized',
		label: 'anonymized to anonymized',
		srcVisibility: 'anonymized',
		dstVisibility: 'anonymized'
	}
];

export type IpMetricKey = 'saIpv4Count' | 'daIpv4Count' | 'saIpv6Count' | 'daIpv6Count';

export type IpMetricFamily = 'ipv4' | 'ipv6';
export type IpMetricVariant = 'source' | 'destination';

export interface IpMetricOption {
	key: IpMetricKey;
	label: string;
	family: IpMetricFamily;
	variant: IpMetricVariant;
}

export const IP_METRIC_OPTIONS: IpMetricOption[] = [
	{ key: 'saIpv4Count', label: 'Source IPv4', family: 'ipv4', variant: 'source' },
	{ key: 'daIpv4Count', label: 'Destination IPv4', family: 'ipv4', variant: 'destination' },
	{ key: 'saIpv6Count', label: 'Source IPv6', family: 'ipv6', variant: 'source' },
	{ key: 'daIpv6Count', label: 'Destination IPv6', family: 'ipv6', variant: 'destination' }
];

export type ProtocolMetricKey = 'uniqueProtocolsIpv4' | 'uniqueProtocolsIpv6';

export interface ProtocolStatsBucket {
	router: string;
	granularity: IpGranularity;
	bucketStart: number;
	bucketEnd: number;
	uniqueProtocolsIpv4: number;
	uniqueProtocolsIpv6: number;
	processedAt?: string;
}

export interface ProtocolStatsResponse {
	buckets: ProtocolStatsBucket[];
	availableGranularities: IpGranularity[];
	requestedRouters: string[];
}

export interface IpStatsCounts {
	saIpv4Count: number;
	daIpv4Count: number;
	saIpv6Count: number;
	daIpv6Count: number;
}

export interface IpStatsBucket extends IpStatsCounts {
	router: string;
	granularity: IpGranularity;
	bucketStart: number;
	bucketEnd: number;
	processedAt?: string;
}

export interface IpStatsResponse {
	buckets: IpStatsBucket[];
	availableGranularities: IpGranularity[];
	requestedRouters: string[];
}

export interface IpChartState {
	startDate: string;
	endDate: string;
	granularity: IpGranularity;
	selectedRouters: string[];
	activeMetrics: IpMetricKey[];
}

export interface SpectrumPoint {
	alpha: number;
	f: number;
}

export interface SpectrumData {
	slug: string;
	router: string;
	filename: string;
	spectrum: SpectrumPoint[];
	metadata: {
		dataSource: string;
		uniqueIPCount?: number;
		pointCount: number;
		addressType: string;
		alphaRange: { min: number; max: number };
	};
}

export interface StructureFunctionPoint {
	q: number;
	tau: number;
	sd: number;
}

export interface StructureFunctionData {
	slug: string;
	router: string;
	filename: string;
	structureFunction: StructureFunctionPoint[];
	metadata: {
		dataSource: string;
		uniqueIPCount?: number;
		pointCount: number;
		addressType: string;
		qRange: { min: number; max: number };
	};
}

export interface StructureStatsBucket {
	bucketStart: number;
	router: string;
	structureSa: StructureFunctionPoint[];
	structureDa: StructureFunctionPoint[];
}

export interface StructureStatsResponse {
	buckets: StructureStatsBucket[];
	requestedRouters: string[];
}

export interface SpectrumStatsBucket {
	bucketStart: number;
	router: string;
	spectrumSa: SpectrumPoint[];
	spectrumDa: SpectrumPoint[];
}

export interface SpectrumStatsResponse {
	buckets: SpectrumStatsBucket[];
	requestedRouters: string[];
}
