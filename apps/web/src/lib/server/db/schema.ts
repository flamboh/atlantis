import { sql } from 'drizzle-orm';
import {
	check,
	index,
	integer,
	primaryKey,
	real,
	sqliteTable,
	text
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
	sortOrder: integer('sort_order').notNull().default(0)
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
		granularity: text('granularity', { enum: ['5m', '30m', '1h', '1d'] }).notNull(),
		bucketStart: integer('bucket_start').notNull(),
		bucketEnd: integer('bucket_end').notNull(),
		ipVersion: integer('ip_version').notNull(),
		srcVisibility: text('src_visibility', { enum: ['all', 'literal', 'anonymized'] }).notNull(),
		dstVisibility: text('dst_visibility', { enum: ['all', 'literal', 'anonymized'] }).notNull(),
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
				table.srcVisibility,
				table.dstVisibility
			]
		}),
		index('idx_traffic_stats_query').on(
			table.granularity,
			table.bucketStart,
			table.sourceId,
			table.ipVersion,
			table.srcVisibility,
			table.dstVisibility
		),
		check('traffic_stats_ip_version_check', sql`${table.ipVersion} IN (4, 6)`)
	]
);

export const protocolStats = sqliteTable(
	'protocol_stats',
	{
		sourceId: text('source_id').notNull(),
		granularity: text('granularity', { enum: ['5m', '30m', '1h', '1d'] }).notNull(),
		bucketStart: integer('bucket_start').notNull(),
		bucketEnd: integer('bucket_end').notNull(),
		ipVersion: integer('ip_version').notNull(),
		srcVisibility: text('src_visibility', { enum: ['all', 'literal', 'anonymized'] }).notNull(),
		dstVisibility: text('dst_visibility', { enum: ['all', 'literal', 'anonymized'] }).notNull(),
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
				table.srcVisibility,
				table.dstVisibility
			]
		}),
		index('idx_protocol_stats_query').on(
			table.granularity,
			table.bucketStart,
			table.sourceId,
			table.ipVersion,
			table.srcVisibility,
			table.dstVisibility
		),
		check('protocol_stats_ip_version_check', sql`${table.ipVersion} IN (4, 6)`)
	]
);

export const addressCountStats = sqliteTable(
	'address_count_stats',
	{
		sourceId: text('source_id').notNull(),
		granularity: text('granularity', { enum: ['5m', '30m', '1h', '1d'] }).notNull(),
		bucketStart: integer('bucket_start').notNull(),
		bucketEnd: integer('bucket_end').notNull(),
		ipVersion: integer('ip_version').notNull(),
		srcVisibility: text('src_visibility', { enum: ['all', 'literal', 'anonymized'] }).notNull(),
		dstVisibility: text('dst_visibility', { enum: ['all', 'literal', 'anonymized'] }).notNull(),
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
				table.srcVisibility,
				table.dstVisibility,
				table.addressSide
			]
		}),
		index('idx_address_count_stats_query').on(
			table.granularity,
			table.bucketStart,
			table.sourceId,
			table.ipVersion,
			table.srcVisibility,
			table.dstVisibility,
			table.addressSide
		),
		check('address_count_stats_ip_version_check', sql`${table.ipVersion} IN (4, 6)`)
	]
);

export const portCountStats = sqliteTable(
	'port_count_stats',
	{
		sourceId: text('source_id').notNull(),
		granularity: text('granularity', { enum: ['5m', '30m', '1h', '1d'] }).notNull(),
		bucketStart: integer('bucket_start').notNull(),
		bucketEnd: integer('bucket_end').notNull(),
		ipVersion: integer('ip_version').notNull(),
		srcVisibility: text('src_visibility', { enum: ['all', 'literal', 'anonymized'] }).notNull(),
		dstVisibility: text('dst_visibility', { enum: ['all', 'literal', 'anonymized'] }).notNull(),
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
				table.srcVisibility,
				table.dstVisibility,
				table.portSide,
				table.portRange
			]
		}),
		index('idx_port_count_stats_query').on(
			table.granularity,
			table.bucketStart,
			table.sourceId,
			table.ipVersion,
			table.srcVisibility,
			table.dstVisibility,
			table.portSide,
			table.portRange
		),
		check('port_count_stats_ip_version_check', sql`${table.ipVersion} IN (4, 6)`)
	]
);

export const addressStructureStats = sqliteTable(
	'address_structure_stats',
	{
		sourceId: text('source_id').notNull(),
		granularity: text('granularity', { enum: ['5m', '30m', '1h', '1d'] }).notNull(),
		bucketStart: integer('bucket_start').notNull(),
		bucketEnd: integer('bucket_end').notNull(),
		ipVersion: integer('ip_version').notNull(),
		srcVisibility: text('src_visibility', { enum: ['all', 'literal', 'anonymized'] }).notNull(),
		dstVisibility: text('dst_visibility', { enum: ['all', 'literal', 'anonymized'] }).notNull(),
		addressSide: text('address_side', { enum: ['source', 'destination'] }).notNull(),
		structureKind: text('structure_kind', {
			enum: ['structure', 'spectrum', 'dimension']
		}).notNull(),
		valuesJson: text('values_json').notNull(),
		metadataJson: text('metadata_json').notNull(),
		processedAt: text('processed_at').default(currentTimestamp)
	},
	(table) => [
		primaryKey({
			columns: [
				table.sourceId,
				table.granularity,
				table.bucketStart,
				table.ipVersion,
				table.srcVisibility,
				table.dstVisibility,
				table.addressSide,
				table.structureKind
			]
		}),
		index('idx_address_structure_stats_query').on(
			table.granularity,
			table.bucketStart,
			table.sourceId,
			table.ipVersion,
			table.srcVisibility,
			table.dstVisibility,
			table.addressSide,
			table.structureKind
		),
		check('address_structure_stats_ip_version_check', sql`${table.ipVersion} IN (4, 6)`)
	]
);
