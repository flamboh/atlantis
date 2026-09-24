export interface DatasetSummary {
	datasetId: string;
	label: string;
	defaultStartDate: string;
	discoveryMode: string;
	hasLocality: boolean;
	isDefault: boolean;
}

export interface DatasetSummariesResponse {
	data: DatasetSummary[] | null;
	error: string | null;
}

export type AlertTail = 'high' | 'low';

export type AlertHorizon = '1h' | '6h' | '24h' | '7d';

export type AlertSort = 'extreme' | 'recent';

export interface AlertFeedAddress {
	address: string;
	tail: AlertTail;
	peakAlpha: number;
	peakWindowStart: number;
	peakR2: number;
	/** Alpha of the address's most recent crossing within the horizon. */
	latestAlpha: number;
	lastSeen: number;
	firstSeen: number;
	timesFlagged: number;
}

export type AlertFeedStatus =
	| { present: false }
	| {
			present: true;
			latestWindowStart: number | null;
			latestWindowEnd: number | null;
			latestAddressCount: number | null;
			latestProcessedAt: number | null;
			thresholds: { high: number; low: number };
	  };

export interface AlertsFeedResponse {
	feed: AlertFeedStatus;
	horizonSeconds: number;
	totalAddresses: number;
	addresses: AlertFeedAddress[];
}

export type CoverageState = 'complete' | 'partial' | 'unknown';

export interface BucketCoverage {
	state: CoverageState;
	observedUnits: number;
	expectedUnits: number;
}

export interface CoverageTimelineBucket {
	bucketStart: number;
	bucketEnd: number;
	coverage: BucketCoverage;
}

export interface CoverageTimeline {
	sourceId: string;
	buckets: CoverageTimelineBucket[];
}

export interface NetflowCoverageResponse {
	timelines: CoverageTimeline[];
	requestedRouters: string[];
}

export type TimeBucket<T> = {
	bucketStart: number;
	bucketEnd: number;
	coverage: BucketCoverage;
	data: T | null;
};

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
	averageDurationMs: number | null;
	averageMinTtl: number | null;
	averageMaxTtl: number | null;
}

export interface NetflowStatsResponse {
	result: TimeBucket<NetflowStatsResult>[];
	availableIpFamilies: NetflowIpFamily[];
}

export type PortSide = 'source' | 'destination';
export type PortRange = 'low' | 'high';

export interface ObservationStats {
	ipFamily: NetflowIpFamily;
	averageDurationMs: number | null;
	averageMinTtl: number | null;
	averageMaxTtl: number | null;
}

export type PortCardinalityCounts = Record<
	Exclude<NetflowIpFamily, 'all'>,
	Record<PortSide, Record<PortRange, number>>
>;

export interface PortCardinalityTimeline {
	sourceId: string;
	buckets: TimeBucket<PortCardinalityCounts>[];
}

export interface FlowCharacteristicsResponse {
	observationBuckets: TimeBucket<ObservationStats[]>[];
	portTimelines: PortCardinalityTimeline[];
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

export const IP_GRANULARITIES = ['5m', '10m', '30m', '1h', '1d'] as const;

export type IpGranularity = (typeof IP_GRANULARITIES)[number];

export const FLOW_DIRECTIONS = ['all', 'ingress', 'egress', 'lateral', 'transit'] as const;
export type FlowDirection = (typeof FLOW_DIRECTIONS)[number];

export type FlowLocality = 'all' | 'internal' | 'external';

export interface FlowLocalityPair {
	srcLocality: FlowLocality;
	dstLocality: FlowLocality;
}

export interface FlowDirectionOption extends FlowLocalityPair {
	value: FlowDirection;
	label: string;
	description: string;
}

export const FLOW_DIRECTION_OPTIONS: FlowDirectionOption[] = [
	{
		value: 'all',
		label: 'All',
		description: 'All traffic',
		srcLocality: 'all',
		dstLocality: 'all'
	},
	{
		value: 'ingress',
		label: 'Ingress',
		description: 'Ingress: external → internal',
		srcLocality: 'external',
		dstLocality: 'internal'
	},
	{
		value: 'egress',
		label: 'Egress',
		description: 'Egress: internal → external',
		srcLocality: 'internal',
		dstLocality: 'external'
	},
	{
		value: 'lateral',
		label: 'Lateral',
		description: 'Lateral: internal → internal',
		srcLocality: 'internal',
		dstLocality: 'internal'
	},
	{
		value: 'transit',
		label: 'Transit',
		description: 'Transit: external → external',
		srcLocality: 'external',
		dstLocality: 'external'
	}
];

const FLOW_DIRECTION_LOCALITIES: Record<FlowDirection, FlowLocalityPair> = Object.fromEntries(
	FLOW_DIRECTION_OPTIONS.map((option) => [
		option.value,
		{ srcLocality: option.srcLocality, dstLocality: option.dstLocality }
	])
) as Record<FlowDirection, FlowLocalityPair>;

export function flowDirectionLocalities(direction: FlowDirection): FlowLocalityPair {
	return FLOW_DIRECTION_LOCALITIES[direction];
}

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
	uniqueProtocolsIpv4: number;
	uniqueProtocolsIpv6: number;
}

export interface ProtocolStatsTimeline {
	router: string;
	buckets: TimeBucket<ProtocolStatsBucket>[];
}

export interface ProtocolStatsResponse {
	timelines: ProtocolStatsTimeline[];
}

export interface IpStatsCounts {
	saIpv4Count: number;
	daIpv4Count: number;
	saIpv6Count: number;
	daIpv6Count: number;
}

export type IpStatsBucket = IpStatsCounts;

export interface IpStatsTimeline {
	router: string;
	buckets: TimeBucket<IpStatsBucket>[];
}

export interface IpStatsResponse {
	timelines: IpStatsTimeline[];
}

export interface IpChartState {
	startDate: string;
	endDate: string;
	granularity: IpGranularity;
	selectedRouters: string[];
	activeMetrics: IpMetricKey[];
}

export const MAAD_IP_VERSIONS = [4, 6] as const;

export type MaadIpVersion = (typeof MAAD_IP_VERSIONS)[number];

export const DEFAULT_MAAD_IP_VERSION: MaadIpVersion = 4;

export interface MaadIpVersionOption {
	value: MaadIpVersion;
	label: string;
}

export const MAAD_IP_VERSION_OPTIONS: MaadIpVersionOption[] = [
	{ value: 4, label: 'IPv4 (/8–/24)' },
	{ value: 6, label: 'IPv6 (/23–/64)' }
];

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
