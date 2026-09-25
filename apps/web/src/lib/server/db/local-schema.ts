export const localSchemaSql = `
	CREATE TABLE IF NOT EXISTS datasets (
		id TEXT PRIMARY KEY NOT NULL,
		label TEXT NOT NULL,
		default_start_date TEXT NOT NULL,
		source_mode TEXT DEFAULT 'static' NOT NULL,
		discovery_mode TEXT DEFAULT 'static' NOT NULL,
		sort_order INTEGER DEFAULT 0 NOT NULL,
		has_locality INTEGER DEFAULT 0 NOT NULL
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

	CREATE TABLE IF NOT EXISTS bucket_coverage (
		source_id TEXT NOT NULL,
		granularity TEXT NOT NULL CHECK(granularity IN ('5m', '10m', '30m', '1h', '1d')),
		bucket_start INTEGER NOT NULL,
		bucket_end INTEGER NOT NULL CHECK(bucket_end > bucket_start),
		coverage_state TEXT NOT NULL CHECK(coverage_state IN ('complete', 'partial', 'unknown')),
		observed_units INTEGER NOT NULL CHECK(observed_units >= 0 AND observed_units <= expected_units),
		expected_units INTEGER NOT NULL CHECK(expected_units > 0),
		rejected_units INTEGER NOT NULL CHECK(rejected_units >= 0 AND rejected_units <= expected_units),
		CHECK(
			(coverage_state = 'complete' AND observed_units = expected_units AND rejected_units = 0)
			OR (coverage_state = 'unknown' AND observed_units = 0 AND rejected_units = 0)
			OR (coverage_state = 'partial'
				AND NOT (observed_units = expected_units AND rejected_units = 0)
				AND NOT (observed_units = 0 AND rejected_units = 0))
		),
		PRIMARY KEY(source_id, granularity, bucket_start)
	) WITHOUT ROWID;

	CREATE TABLE IF NOT EXISTS traffic_stats (
		source_id TEXT NOT NULL,
		granularity TEXT NOT NULL CHECK(granularity IN ('5m', '10m', '30m', '1h', '1d')),
		bucket_start INTEGER NOT NULL,
		bucket_end INTEGER NOT NULL,
		ip_version INTEGER NOT NULL CHECK(ip_version IN (4, 6)),
		src_locality TEXT NOT NULL CHECK(src_locality IN ('all', 'internal', 'external')),
		dst_locality TEXT NOT NULL CHECK(dst_locality IN ('all', 'internal', 'external')),
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
		duration_sum_ms INTEGER NOT NULL,
		duration_count INTEGER NOT NULL,
		average_duration_ms REAL,
		min_ttl_sum INTEGER NOT NULL,
		min_ttl_count INTEGER NOT NULL,
		average_min_ttl REAL,
		max_ttl_sum INTEGER NOT NULL,
		max_ttl_count INTEGER NOT NULL,
		average_max_ttl REAL,
		processed_at TEXT DEFAULT CURRENT_TIMESTAMP,
		PRIMARY KEY(source_id, granularity, bucket_start, ip_version, src_locality, dst_locality)
	);

	CREATE TABLE IF NOT EXISTS protocol_stats (
		source_id TEXT NOT NULL,
		granularity TEXT NOT NULL CHECK(granularity IN ('5m', '10m', '30m', '1h', '1d')),
		bucket_start INTEGER NOT NULL,
		bucket_end INTEGER NOT NULL,
		ip_version INTEGER NOT NULL CHECK(ip_version IN (4, 6)),
		src_locality TEXT NOT NULL CHECK(src_locality IN ('all', 'internal', 'external')),
		dst_locality TEXT NOT NULL CHECK(dst_locality IN ('all', 'internal', 'external')),
		unique_protocols_count INTEGER NOT NULL,
		protocols_list TEXT NOT NULL,
		processed_at TEXT DEFAULT CURRENT_TIMESTAMP,
		PRIMARY KEY(source_id, granularity, bucket_start, ip_version, src_locality, dst_locality)
	);

	CREATE TABLE IF NOT EXISTS address_count_stats (
		source_id TEXT NOT NULL,
		granularity TEXT NOT NULL CHECK(granularity IN ('5m', '10m', '30m', '1h', '1d')),
		bucket_start INTEGER NOT NULL,
		bucket_end INTEGER NOT NULL,
		ip_version INTEGER NOT NULL CHECK(ip_version IN (4, 6)),
		src_locality TEXT NOT NULL CHECK(src_locality IN ('all', 'internal', 'external')),
		dst_locality TEXT NOT NULL CHECK(dst_locality IN ('all', 'internal', 'external')),
		address_side TEXT NOT NULL CHECK(address_side IN ('source', 'destination')),
		unique_address_count INTEGER NOT NULL,
		processed_at TEXT DEFAULT CURRENT_TIMESTAMP,
		PRIMARY KEY(source_id, granularity, bucket_start, ip_version, src_locality, dst_locality, address_side)
	);

	CREATE TABLE IF NOT EXISTS address_maad_stats (
		source_id TEXT NOT NULL,
		granularity TEXT NOT NULL CHECK (granularity IN ('5m', '10m', '30m', '1h', '1d')),
		bucket_start INTEGER NOT NULL,
		bucket_end INTEGER NOT NULL CHECK (bucket_end > bucket_start),
		ip_version INTEGER NOT NULL CHECK (ip_version IN (4, 6)),
		src_locality TEXT NOT NULL CHECK (src_locality IN ('all', 'internal', 'external')),
		dst_locality TEXT NOT NULL CHECK (dst_locality IN ('all', 'internal', 'external')),
		address_side TEXT NOT NULL CHECK (address_side IN ('source', 'destination')),
		measure TEXT NOT NULL CHECK (measure IN ('addresses', 'packets', 'bytes')),
		total_addrs INTEGER NOT NULL CHECK (total_addrs >= 0),
		zero_weight_addrs INTEGER NOT NULL DEFAULT 0
			CHECK (zero_weight_addrs >= 0 AND (measure <> 'addresses' OR zero_weight_addrs = 0)),
		min_prefix_length INTEGER,
		max_prefix_length INTEGER CHECK (max_prefix_length >= min_prefix_length),
		d0 REAL, d1 REAL, d2 REAL,
		tau BLOB,
		tau_sd BLOB CHECK (length(tau_sd) IS length(tau)),
		spectrum BLOB CHECK (length(spectrum) % 8 = 0),
		CHECK (measure = 'addresses' OR spectrum IS NULL),
		CHECK ((tau IS NULL) = (d0 IS NULL))
	);

	CREATE TABLE IF NOT EXISTS maad_q_grid (
		ip_version INTEGER PRIMARY KEY CHECK (ip_version IN (4, 6)),
		q_min REAL NOT NULL,
		q_step REAL NOT NULL CHECK (q_step > 0),
		q_count INTEGER NOT NULL CHECK (q_count > 0)
	);

	CREATE TABLE IF NOT EXISTS port_count_stats (
		source_id TEXT NOT NULL,
		granularity TEXT NOT NULL CHECK(granularity IN ('5m', '10m', '30m', '1h', '1d')),
		bucket_start INTEGER NOT NULL,
		bucket_end INTEGER NOT NULL,
		ip_version INTEGER NOT NULL CHECK(ip_version IN (4, 6)),
		src_locality TEXT NOT NULL CHECK(src_locality IN ('all', 'internal', 'external')),
		dst_locality TEXT NOT NULL CHECK(dst_locality IN ('all', 'internal', 'external')),
		port_side TEXT NOT NULL CHECK(port_side IN ('source', 'destination')),
		port_range TEXT NOT NULL CHECK(port_range IN ('low', 'high')),
		unique_port_count INTEGER NOT NULL,
		processed_at TEXT DEFAULT CURRENT_TIMESTAMP,
		PRIMARY KEY(
			source_id, granularity, bucket_start, ip_version,
			src_locality, dst_locality, port_side, port_range
		)
	);

	CREATE INDEX IF NOT EXISTS idx_processed_inputs_source_bucket
		ON processed_inputs (source_id, bucket_start);
	CREATE INDEX IF NOT EXISTS idx_bucket_coverage_query
		ON bucket_coverage (granularity, bucket_start, source_id);
	CREATE INDEX IF NOT EXISTS idx_traffic_stats_query
		ON traffic_stats (
			granularity, bucket_start, source_id, ip_version,
			src_locality, dst_locality
		);
	CREATE INDEX IF NOT EXISTS idx_traffic_stats_timeseries
		ON traffic_stats (
			source_id, granularity, src_locality, dst_locality,
			bucket_start
		);
	CREATE INDEX IF NOT EXISTS idx_protocol_stats_timeseries
		ON protocol_stats (
			source_id, granularity, src_locality, dst_locality,
			bucket_start
		);
	CREATE INDEX IF NOT EXISTS idx_address_count_stats_query
		ON address_count_stats (
			granularity, bucket_start, source_id, ip_version,
			src_locality, dst_locality, address_side
		);
	CREATE INDEX IF NOT EXISTS idx_address_count_stats_timeseries
		ON address_count_stats (
			source_id, granularity, src_locality, dst_locality,
			bucket_start
		);
	CREATE UNIQUE INDEX IF NOT EXISTS idx_address_maad_stats_key
		ON address_maad_stats (
			source_id, granularity, src_locality, dst_locality,
			ip_version, measure, bucket_start, address_side
		);
	CREATE INDEX IF NOT EXISTS idx_address_maad_stats_bucket
		ON address_maad_stats (granularity, bucket_start);
	CREATE INDEX IF NOT EXISTS idx_port_count_stats_timeseries
		ON port_count_stats (
			source_id, granularity, src_locality, dst_locality,
			bucket_start
		);

	DROP INDEX IF EXISTS idx_protocol_stats_query;
	DROP INDEX IF EXISTS idx_port_count_stats_query;
`;
