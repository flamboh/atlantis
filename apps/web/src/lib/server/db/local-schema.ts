export const localSchemaSql = `
	CREATE TABLE IF NOT EXISTS datasets (
		id TEXT PRIMARY KEY NOT NULL,
		label TEXT NOT NULL,
		default_start_date TEXT NOT NULL,
		source_mode TEXT DEFAULT 'static' NOT NULL,
		discovery_mode TEXT DEFAULT 'static' NOT NULL,
		sort_order INTEGER DEFAULT 0 NOT NULL
	);

	CREATE TABLE IF NOT EXISTS source_members (
		dataset_id TEXT NOT NULL,
		source_id TEXT NOT NULL,
		member_id TEXT NOT NULL,
		PRIMARY KEY(dataset_id, source_id, member_id)
	);

	CREATE TABLE IF NOT EXISTS processed_inputs (
		input_kind TEXT NOT NULL,
		input_locator TEXT NOT NULL,
		source_id TEXT NOT NULL,
		bucket_start INTEGER NOT NULL,
		bucket_end INTEGER NOT NULL,
		status TEXT DEFAULT 'pending' NOT NULL,
		error_message TEXT,
		discovered_at TEXT DEFAULT CURRENT_TIMESTAMP,
		processed_at TEXT,
		PRIMARY KEY(input_kind, input_locator, source_id, bucket_start)
	);

	CREATE TABLE IF NOT EXISTS traffic_stats (
		source_id TEXT NOT NULL,
		granularity TEXT NOT NULL CHECK(granularity IN ('5m', '30m', '1h', '1d')),
		bucket_start INTEGER NOT NULL,
		bucket_end INTEGER NOT NULL,
		ip_version INTEGER NOT NULL CHECK(ip_version IN (4, 6)),
		src_visibility TEXT NOT NULL CHECK(src_visibility IN ('all', 'literal', 'anonymized')),
		dst_visibility TEXT NOT NULL CHECK(dst_visibility IN ('all', 'literal', 'anonymized')),
		flows INTEGER NOT NULL,
		flows_tcp INTEGER NOT NULL,
		flows_udp INTEGER NOT NULL,
		flows_icmp INTEGER NOT NULL,
		flows_other INTEGER NOT NULL,
		packets INTEGER NOT NULL,
		packets_tcp INTEGER NOT NULL,
		packets_udp INTEGER NOT NULL,
		packets_icmp INTEGER NOT NULL,
		packets_other INTEGER NOT NULL,
		bytes INTEGER NOT NULL,
		bytes_tcp INTEGER NOT NULL,
		bytes_udp INTEGER NOT NULL,
		bytes_icmp INTEGER NOT NULL,
		bytes_other INTEGER NOT NULL,
		processed_at TEXT DEFAULT CURRENT_TIMESTAMP,
		PRIMARY KEY(source_id, granularity, bucket_start, ip_version, src_visibility, dst_visibility)
	);

	CREATE TABLE IF NOT EXISTS protocol_stats (
		source_id TEXT NOT NULL,
		granularity TEXT NOT NULL CHECK(granularity IN ('5m', '30m', '1h', '1d')),
		bucket_start INTEGER NOT NULL,
		bucket_end INTEGER NOT NULL,
		ip_version INTEGER NOT NULL CHECK(ip_version IN (4, 6)),
		src_visibility TEXT NOT NULL CHECK(src_visibility IN ('all', 'literal', 'anonymized')),
		dst_visibility TEXT NOT NULL CHECK(dst_visibility IN ('all', 'literal', 'anonymized')),
		unique_protocols_count INTEGER NOT NULL,
		protocols_list TEXT NOT NULL,
		processed_at TEXT DEFAULT CURRENT_TIMESTAMP,
		PRIMARY KEY(source_id, granularity, bucket_start, ip_version, src_visibility, dst_visibility)
	);

	CREATE TABLE IF NOT EXISTS address_count_stats (
		source_id TEXT NOT NULL,
		granularity TEXT NOT NULL CHECK(granularity IN ('5m', '30m', '1h', '1d')),
		bucket_start INTEGER NOT NULL,
		bucket_end INTEGER NOT NULL,
		ip_version INTEGER NOT NULL CHECK(ip_version IN (4, 6)),
		src_visibility TEXT NOT NULL CHECK(src_visibility IN ('all', 'literal', 'anonymized')),
		dst_visibility TEXT NOT NULL CHECK(dst_visibility IN ('all', 'literal', 'anonymized')),
		address_side TEXT NOT NULL CHECK(address_side IN ('source', 'destination')),
		unique_address_count INTEGER NOT NULL,
		processed_at TEXT DEFAULT CURRENT_TIMESTAMP,
		PRIMARY KEY(source_id, granularity, bucket_start, ip_version, src_visibility, dst_visibility, address_side)
	);

	CREATE TABLE IF NOT EXISTS address_structure_stats (
		source_id TEXT NOT NULL,
		granularity TEXT NOT NULL CHECK(granularity IN ('5m', '30m', '1h', '1d')),
		bucket_start INTEGER NOT NULL,
		bucket_end INTEGER NOT NULL,
		ip_version INTEGER NOT NULL CHECK(ip_version IN (4, 6)),
		src_visibility TEXT NOT NULL CHECK(src_visibility IN ('all', 'literal', 'anonymized')),
		dst_visibility TEXT NOT NULL CHECK(dst_visibility IN ('all', 'literal', 'anonymized')),
		address_side TEXT NOT NULL CHECK(address_side IN ('source', 'destination')),
		structure_kind TEXT NOT NULL CHECK(structure_kind IN ('structure', 'spectrum', 'dimension')),
		values_json TEXT NOT NULL,
		metadata_json TEXT NOT NULL,
		processed_at TEXT DEFAULT CURRENT_TIMESTAMP,
		PRIMARY KEY(
			source_id, granularity, bucket_start, ip_version,
			src_visibility, dst_visibility, address_side, structure_kind
		)
	);

	CREATE INDEX IF NOT EXISTS idx_processed_inputs_source_bucket
		ON processed_inputs (source_id, bucket_start);
	CREATE INDEX IF NOT EXISTS idx_traffic_stats_query
		ON traffic_stats (
			granularity, bucket_start, source_id, ip_version,
			src_visibility, dst_visibility
		);
	CREATE INDEX IF NOT EXISTS idx_protocol_stats_query
		ON protocol_stats (
			granularity, bucket_start, source_id, ip_version,
			src_visibility, dst_visibility
		);
	CREATE INDEX IF NOT EXISTS idx_address_count_stats_query
		ON address_count_stats (
			granularity, bucket_start, source_id, ip_version,
			src_visibility, dst_visibility, address_side
		);
	CREATE INDEX IF NOT EXISTS idx_address_structure_stats_query
		ON address_structure_stats (
			granularity, bucket_start, source_id, ip_version,
			src_visibility, dst_visibility, address_side, structure_kind
		);
`;
