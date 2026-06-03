import { sql } from 'drizzle-orm';
import { check, index, integer, primaryKey, sqliteTable, text } from 'drizzle-orm/sqlite-core';

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

export const processedInputsV2 = sqliteTable(
	'processed_inputs_v2',
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
		index('idx_processed_inputs_v2_source_bucket').on(table.sourceId, table.bucketStart)
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
		bytesOther: integer('bytes_other').notNull()
	};
}

export const netflowStatsV2 = sqliteTable(
	'netflow_stats_v2',
	{
		sourceId: text('source_id').notNull(),
		granularity: text('granularity', { enum: ['5m', '30m', '1h', '1d'] }).notNull(),
		bucketStart: integer('bucket_start').notNull(),
		bucketEnd: integer('bucket_end').notNull(),
		ipVersion: integer('ip_version').notNull(),
		...netflowMetricColumns(),
		processedAt: text('processed_at').default(currentTimestamp)
	},
	(table) => [
		primaryKey({
			columns: [table.sourceId, table.granularity, table.bucketStart, table.ipVersion]
		}),
		index('idx_netflow_stats_v2_granularity_bucket_source').on(
			table.granularity,
			table.bucketStart,
			table.sourceId,
			table.ipVersion
		),
		check('netflow_stats_v2_ip_version_check', sql`${table.ipVersion} IN (4, 6)`)
	]
);

export const ipStatsV2 = sqliteTable(
	'ip_stats_v2',
	{
		sourceId: text('source_id').notNull(),
		granularity: text('granularity', { enum: ['5m', '30m', '1h', '1d'] }).notNull(),
		bucketStart: integer('bucket_start').notNull(),
		bucketEnd: integer('bucket_end').notNull(),
		saIpv4Count: integer('sa_ipv4_count').notNull(),
		daIpv4Count: integer('da_ipv4_count').notNull(),
		saIpv6Count: integer('sa_ipv6_count').notNull(),
		daIpv6Count: integer('da_ipv6_count').notNull(),
		processedAt: text('processed_at').default(currentTimestamp)
	},
	(table) => [
		primaryKey({ columns: [table.sourceId, table.granularity, table.bucketStart] }),
		index('idx_ip_stats_v2_granularity_bucket_source').on(
			table.granularity,
			table.bucketStart,
			table.sourceId
		)
	]
);

export const protocolStatsV2 = sqliteTable(
	'protocol_stats_v2',
	{
		sourceId: text('source_id').notNull(),
		granularity: text('granularity', { enum: ['5m', '30m', '1h', '1d'] }).notNull(),
		bucketStart: integer('bucket_start').notNull(),
		bucketEnd: integer('bucket_end').notNull(),
		uniqueProtocolsCountIpv4: integer('unique_protocols_count_ipv4').notNull(),
		uniqueProtocolsCountIpv6: integer('unique_protocols_count_ipv6').notNull(),
		protocolsListIpv4: text('protocols_list_ipv4').notNull(),
		protocolsListIpv6: text('protocols_list_ipv6').notNull(),
		processedAt: text('processed_at').default(currentTimestamp)
	},
	(table) => [
		primaryKey({ columns: [table.sourceId, table.granularity, table.bucketStart] }),
		index('idx_protocol_stats_v2_granularity_bucket_source').on(
			table.granularity,
			table.bucketStart,
			table.sourceId
		)
	]
);

export const structureStatsV2 = sqliteTable(
	'structure_stats_v2',
	{
		sourceId: text('source_id').notNull(),
		granularity: text('granularity', { enum: ['5m', '30m', '1h', '1d'] }).notNull(),
		bucketStart: integer('bucket_start').notNull(),
		bucketEnd: integer('bucket_end').notNull(),
		ipVersion: integer('ip_version').notNull(),
		structureJsonSa: text('structure_json_sa').notNull(),
		structureJsonDa: text('structure_json_da').notNull(),
		metadataJsonSa: text('metadata_json_sa').notNull(),
		metadataJsonDa: text('metadata_json_da').notNull(),
		processedAt: text('processed_at').default(currentTimestamp)
	},
	(table) => [
		primaryKey({
			columns: [table.sourceId, table.granularity, table.bucketStart, table.ipVersion]
		}),
		index('idx_structure_stats_v2_granularity_bucket_source').on(
			table.granularity,
			table.bucketStart,
			table.sourceId,
			table.ipVersion
		),
		check('structure_stats_v2_ip_version_check', sql`${table.ipVersion} IN (4, 6)`)
	]
);

export const spectrumStatsV2 = sqliteTable(
	'spectrum_stats_v2',
	{
		sourceId: text('source_id').notNull(),
		granularity: text('granularity', { enum: ['5m', '30m', '1h', '1d'] }).notNull(),
		bucketStart: integer('bucket_start').notNull(),
		bucketEnd: integer('bucket_end').notNull(),
		ipVersion: integer('ip_version').notNull(),
		spectrumJsonSa: text('spectrum_json_sa').notNull(),
		spectrumJsonDa: text('spectrum_json_da').notNull(),
		metadataJsonSa: text('metadata_json_sa').notNull(),
		metadataJsonDa: text('metadata_json_da').notNull(),
		processedAt: text('processed_at').default(currentTimestamp)
	},
	(table) => [
		primaryKey({
			columns: [table.sourceId, table.granularity, table.bucketStart, table.ipVersion]
		}),
		index('idx_spectrum_stats_v2_granularity_bucket_source').on(
			table.granularity,
			table.bucketStart,
			table.sourceId,
			table.ipVersion
		),
		check('spectrum_stats_v2_ip_version_check', sql`${table.ipVersion} IN (4, 6)`)
	]
);

export const dimensionStatsV2 = sqliteTable(
	'dimension_stats_v2',
	{
		sourceId: text('source_id').notNull(),
		granularity: text('granularity', { enum: ['5m', '30m', '1h', '1d'] }).notNull(),
		bucketStart: integer('bucket_start').notNull(),
		bucketEnd: integer('bucket_end').notNull(),
		ipVersion: integer('ip_version').notNull(),
		dimensionsJsonSa: text('dimensions_json_sa').notNull(),
		dimensionsJsonDa: text('dimensions_json_da').notNull(),
		metadataJsonSa: text('metadata_json_sa').notNull(),
		metadataJsonDa: text('metadata_json_da').notNull(),
		processedAt: text('processed_at').default(currentTimestamp)
	},
	(table) => [
		primaryKey({
			columns: [table.sourceId, table.granularity, table.bucketStart, table.ipVersion]
		}),
		index('idx_dimension_stats_v2_granularity_bucket_source').on(
			table.granularity,
			table.bucketStart,
			table.sourceId,
			table.ipVersion
		),
		check('dimension_stats_v2_ip_version_check', sql`${table.ipVersion} IN (4, 6)`)
	]
);
