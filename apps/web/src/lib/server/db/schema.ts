import { sql } from 'drizzle-orm';
import {
	blob,
	check,
	index,
	integer,
	primaryKey,
	real,
	sqliteTable,
	text,
	uniqueIndex
} from 'drizzle-orm/sqlite-core';

const currentTimestamp = sql`CURRENT_TIMESTAMP`;
export const datasets = sqliteTable('datasets', {
	id: text('id').primaryKey(),
	label: text('label').notNull(),
	defaultStartDate: text('default_start_date').notNull(),
	sourceMode: text('source_mode', { enum: ['static', 'subdirs'] })
		.notNull()
		.default('static'),
	discoveryMode: text('discovery_mode', { enum: ['static', 'live'] })
		.notNull()
		.default('static'),
	sortOrder: integer('sort_order').notNull().default(0),
	hasLocality: integer('has_locality', { mode: 'boolean' }).notNull().default(false),
	maadInternalSide: integer('maad_internal_side', { mode: 'boolean' }).notNull().default(true)
});

export const sourceMembers = sqliteTable(
	'source_members',
	{
		datasetId: text('dataset_id').notNull(),
		sourceId: text('source_id').notNull(),
		memberId: text('member_id').notNull()
	},
	(table) => [
		primaryKey({
			columns: [table.datasetId, table.sourceId, table.memberId]
		})
	]
);

export const processedInputs = sqliteTable(
	'processed_inputs',
	{
		inputKind: text('input_kind', { enum: ['nfcapd', 'csv'] }).notNull(),
		inputLocator: text('input_locator').notNull(),
		sourceId: text('source_id').notNull(),
		bucketStart: integer('bucket_start').notNull(),
		bucketEnd: integer('bucket_end').notNull(),
		status: text('status', { enum: ['pending', 'processed', 'failed'] })
			.notNull()
			.default('pending'),
		errorMessage: text('error_message'),
		discoveredAt: text('discovered_at').default(currentTimestamp),
		processedAt: text('processed_at')
	},
	(table) => [
		primaryKey({
			columns: [table.inputKind, table.inputLocator, table.sourceId, table.bucketStart]
		}),
		index('idx_processed_inputs_source_bucket').on(table.sourceId, table.bucketStart)
	]
);

export const bucketCoverage = sqliteTable(
	'bucket_coverage',
	{
		sourceId: text('source_id').notNull(),
		granularity: text('granularity', { enum: ['5m', '10m', '30m', '1h', '1d'] }).notNull(),
		bucketStart: integer('bucket_start').notNull(),
		bucketEnd: integer('bucket_end').notNull(),
		coverageState: text('coverage_state', {
			enum: ['complete', 'partial', 'unknown']
		}).notNull(),
		observedUnits: integer('observed_units').notNull(),
		expectedUnits: integer('expected_units').notNull(),
		rejectedUnits: integer('rejected_units').notNull()
	},
	(table) => [
		primaryKey({ columns: [table.sourceId, table.granularity, table.bucketStart] }),
		index('idx_bucket_coverage_query').on(table.granularity, table.bucketStart, table.sourceId),
		check('bucket_coverage_interval_check', sql`${table.bucketEnd} > ${table.bucketStart}`),
		check('bucket_coverage_expected_check', sql`${table.expectedUnits} > 0`),
		check(
			'bucket_coverage_observed_check',
			sql`${table.observedUnits} >= 0 AND ${table.observedUnits} <= ${table.expectedUnits}`
		),
		check(
			'bucket_coverage_rejected_check',
			sql`${table.rejectedUnits} >= 0 AND ${table.rejectedUnits} <= ${table.expectedUnits}`
		),
		check(
			'bucket_coverage_state_check',
			sql`(${table.coverageState} = 'complete' AND ${table.observedUnits} = ${table.expectedUnits} AND ${table.rejectedUnits} = 0) OR (${table.coverageState} = 'unknown' AND ${table.observedUnits} = 0 AND ${table.rejectedUnits} = 0) OR (${table.coverageState} = 'partial' AND NOT (${table.observedUnits} = ${table.expectedUnits} AND ${table.rejectedUnits} = 0) AND NOT (${table.observedUnits} = 0 AND ${table.rejectedUnits} = 0))`
		)
	]
);

function netflowMetricColumns() {
	return {
		flows: integer('flows').notNull(),
		flowsTcp: integer('flows_tcp').notNull(),
		flowsUdp: integer('flows_udp').notNull(),
		flowsIcmp: integer('flows_icmp').notNull(),
		flowsOther: integer('flows_other').notNull(),
		packets: integer('packets').notNull(),
		packetsTcp: integer('packets_tcp').notNull(),
		packetsUdp: integer('packets_udp').notNull(),
		packetsIcmp: integer('packets_icmp').notNull(),
		packetsOther: integer('packets_other').notNull(),
		bytes: integer('bytes').notNull(),
		bytesTcp: integer('bytes_tcp').notNull(),
		bytesUdp: integer('bytes_udp').notNull(),
		bytesIcmp: integer('bytes_icmp').notNull(),
		bytesOther: integer('bytes_other').notNull(),
		durationSumMs: integer('duration_sum_ms').notNull(),
		durationCount: integer('duration_count').notNull(),
		averageDurationMs: real('average_duration_ms'),
		minTtlSum: integer('min_ttl_sum').notNull(),
		minTtlCount: integer('min_ttl_count').notNull(),
		averageMinTtl: real('average_min_ttl'),
		maxTtlSum: integer('max_ttl_sum').notNull(),
		maxTtlCount: integer('max_ttl_count').notNull(),
		averageMaxTtl: real('average_max_ttl')
	};
}

export const trafficStats = sqliteTable(
	'traffic_stats',
	{
		sourceId: text('source_id').notNull(),
		granularity: text('granularity', { enum: ['5m', '10m', '30m', '1h', '1d'] }).notNull(),
		bucketStart: integer('bucket_start').notNull(),
		bucketEnd: integer('bucket_end').notNull(),
		ipVersion: integer('ip_version').notNull(),
		srcLocality: text('src_locality', { enum: ['all', 'internal', 'external'] }).notNull(),
		dstLocality: text('dst_locality', { enum: ['all', 'internal', 'external'] }).notNull(),
		...netflowMetricColumns(),
		processedAt: text('processed_at').default(currentTimestamp)
	},
	(table) => [
		primaryKey({
			columns: [
				table.sourceId,
				table.granularity,
				table.bucketStart,
				table.ipVersion,
				table.srcLocality,
				table.dstLocality
			]
		}),
		index('idx_traffic_stats_query').on(
			table.granularity,
			table.bucketStart,
			table.sourceId,
			table.ipVersion,
			table.srcLocality,
			table.dstLocality
		),
		index('idx_traffic_stats_timeseries').on(
			table.sourceId,
			table.granularity,
			table.srcLocality,
			table.dstLocality,
			table.bucketStart
		),
		check('traffic_stats_ip_version_check', sql`${table.ipVersion} IN (4, 6)`),
		check(
			'traffic_stats_src_locality_check',
			sql`${table.srcLocality} IN ('all', 'internal', 'external')`
		),
		check(
			'traffic_stats_dst_locality_check',
			sql`${table.dstLocality} IN ('all', 'internal', 'external')`
		)
	]
);

export const protocolStats = sqliteTable(
	'protocol_stats',
	{
		sourceId: text('source_id').notNull(),
		granularity: text('granularity', { enum: ['5m', '10m', '30m', '1h', '1d'] }).notNull(),
		bucketStart: integer('bucket_start').notNull(),
		bucketEnd: integer('bucket_end').notNull(),
		ipVersion: integer('ip_version').notNull(),
		srcLocality: text('src_locality', { enum: ['all', 'internal', 'external'] }).notNull(),
		dstLocality: text('dst_locality', { enum: ['all', 'internal', 'external'] }).notNull(),
		uniqueProtocolsCount: integer('unique_protocols_count').notNull(),
		protocolsList: text('protocols_list').notNull(),
		processedAt: text('processed_at').default(currentTimestamp)
	},
	(table) => [
		primaryKey({
			columns: [
				table.sourceId,
				table.granularity,
				table.bucketStart,
				table.ipVersion,
				table.srcLocality,
				table.dstLocality
			]
		}),
		index('idx_protocol_stats_timeseries').on(
			table.sourceId,
			table.granularity,
			table.srcLocality,
			table.dstLocality,
			table.bucketStart
		),
		check('protocol_stats_ip_version_check', sql`${table.ipVersion} IN (4, 6)`),
		check(
			'protocol_stats_src_locality_check',
			sql`${table.srcLocality} IN ('all', 'internal', 'external')`
		),
		check(
			'protocol_stats_dst_locality_check',
			sql`${table.dstLocality} IN ('all', 'internal', 'external')`
		)
	]
);

export const addressCountStats = sqliteTable(
	'address_count_stats',
	{
		sourceId: text('source_id').notNull(),
		granularity: text('granularity', { enum: ['5m', '10m', '30m', '1h', '1d'] }).notNull(),
		bucketStart: integer('bucket_start').notNull(),
		bucketEnd: integer('bucket_end').notNull(),
		ipVersion: integer('ip_version').notNull(),
		srcLocality: text('src_locality', { enum: ['all', 'internal', 'external'] }).notNull(),
		dstLocality: text('dst_locality', { enum: ['all', 'internal', 'external'] }).notNull(),
		addressSide: text('address_side', { enum: ['source', 'destination'] }).notNull(),
		uniqueAddressCount: integer('unique_address_count').notNull(),
		processedAt: text('processed_at').default(currentTimestamp)
	},
	(table) => [
		primaryKey({
			columns: [
				table.sourceId,
				table.granularity,
				table.bucketStart,
				table.ipVersion,
				table.srcLocality,
				table.dstLocality,
				table.addressSide
			]
		}),
		index('idx_address_count_stats_query').on(
			table.granularity,
			table.bucketStart,
			table.sourceId,
			table.ipVersion,
			table.srcLocality,
			table.dstLocality,
			table.addressSide
		),
		index('idx_address_count_stats_timeseries').on(
			table.sourceId,
			table.granularity,
			table.srcLocality,
			table.dstLocality,
			table.bucketStart
		),
		check('address_count_stats_ip_version_check', sql`${table.ipVersion} IN (4, 6)`),
		check(
			'address_count_stats_src_locality_check',
			sql`${table.srcLocality} IN ('all', 'internal', 'external')`
		),
		check(
			'address_count_stats_dst_locality_check',
			sql`${table.dstLocality} IN ('all', 'internal', 'external')`
		)
	]
);

export const portCountStats = sqliteTable(
	'port_count_stats',
	{
		sourceId: text('source_id').notNull(),
		granularity: text('granularity', { enum: ['5m', '10m', '30m', '1h', '1d'] }).notNull(),
		bucketStart: integer('bucket_start').notNull(),
		bucketEnd: integer('bucket_end').notNull(),
		ipVersion: integer('ip_version').notNull(),
		srcLocality: text('src_locality', { enum: ['all', 'internal', 'external'] }).notNull(),
		dstLocality: text('dst_locality', { enum: ['all', 'internal', 'external'] }).notNull(),
		portSide: text('port_side', { enum: ['source', 'destination'] }).notNull(),
		portRange: text('port_range', { enum: ['low', 'high'] }).notNull(),
		uniquePortCount: integer('unique_port_count').notNull(),
		processedAt: text('processed_at').default(currentTimestamp)
	},
	(table) => [
		primaryKey({
			columns: [
				table.sourceId,
				table.granularity,
				table.bucketStart,
				table.ipVersion,
				table.srcLocality,
				table.dstLocality,
				table.portSide,
				table.portRange
			]
		}),
		index('idx_port_count_stats_timeseries').on(
			table.sourceId,
			table.granularity,
			table.srcLocality,
			table.dstLocality,
			table.bucketStart
		),
		check('port_count_stats_ip_version_check', sql`${table.ipVersion} IN (4, 6)`),
		check(
			'port_count_stats_src_locality_check',
			sql`${table.srcLocality} IN ('all', 'internal', 'external')`
		),
		check(
			'port_count_stats_dst_locality_check',
			sql`${table.dstLocality} IN ('all', 'internal', 'external')`
		)
	]
);

export const addressMaadStats = sqliteTable(
	'address_maad_stats',
	{
		sourceId: text('source_id').notNull(),
		granularity: text('granularity', { enum: ['5m', '10m', '30m', '1h', '1d'] }).notNull(),
		bucketStart: integer('bucket_start').notNull(),
		bucketEnd: integer('bucket_end').notNull(),
		ipVersion: integer('ip_version').notNull(),
		srcLocality: text('src_locality', { enum: ['all', 'internal', 'external'] }).notNull(),
		dstLocality: text('dst_locality', { enum: ['all', 'internal', 'external'] }).notNull(),
		addressSide: text('address_side', { enum: ['source', 'destination'] }).notNull(),
		measure: text('measure', { enum: ['addresses', 'packets', 'bytes'] }).notNull(),
		totalAddrs: integer('total_addrs').notNull(),
		zeroWeightAddrs: integer('zero_weight_addrs').notNull().default(0),
		minPrefixLength: integer('min_prefix_length'),
		maxPrefixLength: integer('max_prefix_length'),
		d0: real('d0'),
		d1: real('d1'),
		d2: real('d2'),
		tau: blob('tau', { mode: 'buffer' }),
		tauSd: blob('tau_sd', { mode: 'buffer' }),
		spectrum: blob('spectrum', { mode: 'buffer' })
	},
	(table) => [
		uniqueIndex('idx_address_maad_stats_key').on(
			table.sourceId,
			table.granularity,
			table.srcLocality,
			table.dstLocality,
			table.ipVersion,
			table.measure,
			table.bucketStart,
			table.addressSide
		),
		index('idx_address_maad_stats_bucket').on(table.granularity, table.bucketStart),
		check('address_maad_stats_ip_version_check', sql`${table.ipVersion} IN (4, 6)`),
		check(
			'address_maad_stats_src_locality_check',
			sql`${table.srcLocality} IN ('all', 'internal', 'external')`
		),
		check(
			'address_maad_stats_dst_locality_check',
			sql`${table.dstLocality} IN ('all', 'internal', 'external')`
		),
		check(
			'address_maad_stats_measure_check',
			sql`${table.measure} IN ('addresses', 'packets', 'bytes')`
		),
		check('address_maad_stats_bucket_check', sql`${table.bucketEnd} > ${table.bucketStart}`),
		check('address_maad_stats_total_addrs_check', sql`${table.totalAddrs} >= 0`),
		check(
			'address_maad_stats_zero_weight_addrs_check',
			sql`${table.zeroWeightAddrs} >= 0 AND (${table.measure} <> 'addresses' OR ${table.zeroWeightAddrs} = 0)`
		),
		check(
			'address_maad_stats_prefix_length_check',
			sql`${table.maxPrefixLength} >= ${table.minPrefixLength}`
		),
		check(
			'address_maad_stats_tau_check',
			sql`typeof(${table.tau}) IN ('null', 'blob') AND length(${table.tau}) > 0 AND length(${table.tau}) % 4 = 0`
		),
		check(
			'address_maad_stats_tau_sd_check',
			sql`typeof(${table.tauSd}) = typeof(${table.tau}) AND length(${table.tauSd}) IS length(${table.tau})`
		),
		check(
			'address_maad_stats_spectrum_check',
			sql`typeof(${table.spectrum}) IN ('null', 'blob') AND length(${table.spectrum}) % 8 = 0`
		),
		check(
			'address_maad_stats_measure_spectrum_check',
			sql`${table.measure} = 'addresses' OR ${table.spectrum} IS NULL`
		),
		check('address_maad_stats_tau_d0_check', sql`(${table.tau} IS NULL) = (${table.d0} IS NULL)`)
	]
);

export const addressConcentrationStats = sqliteTable(
	'address_concentration_stats',
	{
		sourceId: text('source_id').notNull(),
		granularity: text('granularity', { enum: ['5m', '10m', '30m', '1h', '1d'] }).notNull(),
		bucketStart: integer('bucket_start').notNull(),
		bucketEnd: integer('bucket_end').notNull(),
		ipVersion: integer('ip_version').notNull(),
		srcLocality: text('src_locality', { enum: ['all', 'internal', 'external'] }).notNull(),
		dstLocality: text('dst_locality', { enum: ['all', 'internal', 'external'] }).notNull(),
		addressSide: text('address_side', { enum: ['source', 'destination'] }).notNull(),
		measure: text('measure', { enum: ['addresses', 'packets', 'bytes'] }).notNull(),
		weightTotal: real('weight_total').notNull(),
		entryCount: integer('entry_count').notNull(),
		hhi: real('hhi'),
		top1Share: real('top1_share'),
		top10Share: real('top10_share'),
		top100Share: real('top100_share'),
		entropyP8: real('entropy_p8'),
		entropyP16: real('entropy_p16'),
		entropyP24: real('entropy_p24'),
		entropyP32: real('entropy_p32'),
		entropyP48: real('entropy_p48'),
		entropyP64: real('entropy_p64'),
		entropyP128: real('entropy_p128')
	},
	(table) => [
		uniqueIndex('idx_address_concentration_stats_key').on(
			table.sourceId,
			table.granularity,
			table.srcLocality,
			table.dstLocality,
			table.ipVersion,
			table.measure,
			table.bucketStart,
			table.addressSide
		),
		index('idx_address_concentration_stats_bucket').on(table.granularity, table.bucketStart),
		check(
			'address_concentration_stats_granularity_check',
			sql`${table.granularity} IN ('5m', '10m', '30m', '1h', '1d')`
		),
		check(
			'address_concentration_stats_bucket_check',
			sql`${table.bucketEnd} > ${table.bucketStart}`
		),
		check('address_concentration_stats_ip_version_check', sql`${table.ipVersion} IN (4, 6)`),
		check(
			'address_concentration_stats_src_locality_check',
			sql`${table.srcLocality} IN ('all', 'internal', 'external')`
		),
		check(
			'address_concentration_stats_dst_locality_check',
			sql`${table.dstLocality} IN ('all', 'internal', 'external')`
		),
		check(
			'address_concentration_stats_address_side_check',
			sql`${table.addressSide} IN ('source', 'destination')`
		),
		check(
			'address_concentration_stats_measure_check',
			sql`${table.measure} IN ('addresses', 'packets', 'bytes')`
		),
		check(
			'address_concentration_stats_weight_total_check',
			sql`${table.weightTotal} >= 0 AND ${table.weightTotal} <= 1.7976931348623157e308`
		),
		check('address_concentration_stats_entry_count_check', sql`${table.entryCount} >= 0`),
		check(
			'address_concentration_stats_empty_weight_check',
			sql`(${table.entryCount} = 0) = (${table.weightTotal} = 0)`
		),
		check(
			'address_concentration_stats_minimum_count_check',
			sql`(${table.hhi} IS NOT NULL) = (${table.entryCount} >= 2)`
		),
		check('address_concentration_stats_hhi_check', sql`${table.hhi} > 0 AND ${table.hhi} <= 1`),
		check(
			'address_concentration_stats_top1_share_check',
			sql`${table.top1Share} >= 0 AND ${table.top1Share} <= 1`
		),
		check(
			'address_concentration_stats_top10_share_check',
			sql`${table.top10Share} >= 0 AND ${table.top10Share} <= 1`
		),
		check(
			'address_concentration_stats_top100_share_check',
			sql`${table.top100Share} >= 0 AND ${table.top100Share} <= 1`
		),
		check(
			'address_concentration_stats_entropy_p8_check',
			sql`${table.entropyP8} >= 0 AND ${table.entropyP8} <= 1.7976931348623157e308`
		),
		check(
			'address_concentration_stats_entropy_p16_check',
			sql`${table.entropyP16} >= 0 AND ${table.entropyP16} <= 1.7976931348623157e308`
		),
		check(
			'address_concentration_stats_entropy_p24_check',
			sql`${table.entropyP24} >= 0 AND ${table.entropyP24} <= 1.7976931348623157e308`
		),
		check(
			'address_concentration_stats_entropy_p32_check',
			sql`${table.entropyP32} >= 0 AND ${table.entropyP32} <= 1.7976931348623157e308`
		),
		check(
			'address_concentration_stats_entropy_p48_check',
			sql`${table.entropyP48} >= 0 AND ${table.entropyP48} <= 1.7976931348623157e308`
		),
		check(
			'address_concentration_stats_entropy_p64_check',
			sql`${table.entropyP64} >= 0 AND ${table.entropyP64} <= 1.7976931348623157e308`
		),
		check(
			'address_concentration_stats_entropy_p128_check',
			sql`${table.entropyP128} >= 0 AND ${table.entropyP128} <= 1.7976931348623157e308`
		),
		check(
			'address_concentration_stats_top1_share_presence_check',
			sql`(${table.top1Share} IS NULL) = (${table.hhi} IS NULL)`
		),
		check(
			'address_concentration_stats_top10_share_presence_check',
			sql`(${table.top10Share} IS NULL) = (${table.hhi} IS NULL)`
		),
		check(
			'address_concentration_stats_top100_share_presence_check',
			sql`(${table.top100Share} IS NULL) = (${table.hhi} IS NULL)`
		),
		check(
			'address_concentration_stats_entropy_p32_presence_check',
			sql`(${table.entropyP32} IS NULL) = (${table.hhi} IS NULL)`
		),
		check(
			'address_concentration_stats_entropy_p8_presence_check',
			sql`(${table.entropyP8} IS NOT NULL) = (${table.ipVersion} = 4 AND ${table.hhi} IS NOT NULL)`
		),
		check(
			'address_concentration_stats_entropy_p16_presence_check',
			sql`(${table.entropyP16} IS NOT NULL) = (${table.ipVersion} = 4 AND ${table.hhi} IS NOT NULL)`
		),
		check(
			'address_concentration_stats_entropy_p24_presence_check',
			sql`(${table.entropyP24} IS NOT NULL) = (${table.ipVersion} = 4 AND ${table.hhi} IS NOT NULL)`
		),
		check(
			'address_concentration_stats_entropy_p48_presence_check',
			sql`(${table.entropyP48} IS NOT NULL) = (${table.ipVersion} = 6 AND ${table.hhi} IS NOT NULL)`
		),
		check(
			'address_concentration_stats_entropy_p64_presence_check',
			sql`(${table.entropyP64} IS NOT NULL) = (${table.ipVersion} = 6 AND ${table.hhi} IS NOT NULL)`
		),
		check(
			'address_concentration_stats_entropy_p128_presence_check',
			sql`(${table.entropyP128} IS NOT NULL) = (${table.ipVersion} = 6 AND ${table.hhi} IS NOT NULL)`
		),
		check(
			'address_concentration_stats_top_order_check',
			sql`${table.top10Share} >= ${table.top1Share} AND ${table.top100Share} >= ${table.top10Share}`
		)
	]
);

export const maadQGrid = sqliteTable(
	'maad_q_grid',
	{
		ipVersion: integer('ip_version').primaryKey(),
		qMin: real('q_min').notNull(),
		qStep: real('q_step').notNull(),
		qCount: integer('q_count').notNull()
	},
	(table) => [
		check('maad_q_grid_ip_version_check', sql`${table.ipVersion} IN (4, 6)`),
		check('maad_q_grid_q_step_check', sql`${table.qStep} > 0`),
		check('maad_q_grid_q_count_check', sql`${table.qCount} > 0`)
	]
);
