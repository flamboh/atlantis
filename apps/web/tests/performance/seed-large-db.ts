import fs from 'node:fs';
import path from 'node:path';
import Database from 'better-sqlite3';
// Node's type-stripping runner requires the extension; this script is not bundled by Vite.
// @ts-expect-error allowImportingTsExtensions is intentionally disabled for application code.
import { localSchemaSql } from '../../src/lib/server/db/local-schema.ts';

const outputPath = path.resolve(process.argv[2] ?? '/tmp/atlantis-web-perf.sqlite');
const sourceCount = Number(process.env.ATLANTIS_PERF_SOURCES ?? 16);
const fiveMinuteBucketCount = Number(process.env.ATLANTIS_PERF_BUCKETS ?? 17_280);
const firstBucket = 1_735_718_400; // 2025-01-01 00:00:00 PST
const profilerStartBucket = firstBucket + 31 * 86_400; // 2025-02-01 00:00:00 PST
const profilerEndBucket = profilerStartBucket + 14 * 86_400; // 2025-02-15 00:00:00 PST

if (!Number.isInteger(sourceCount) || sourceCount < 1) {
	throw new Error('ATLANTIS_PERF_SOURCES must be a positive integer');
}
if (!Number.isInteger(fiveMinuteBucketCount) || fiveMinuteBucketCount < 288) {
	throw new Error('ATLANTIS_PERF_BUCKETS must be an integer of at least 288');
}

fs.rmSync(outputPath, { force: true });
const db = new Database(outputPath);
db.pragma('journal_mode = OFF');
db.pragma('synchronous = OFF');
db.pragma('temp_store = MEMORY');
db.exec(localSchemaSql);

for (const index of [
	'idx_processed_inputs_source_bucket',
	'idx_bucket_coverage_query',
	'idx_traffic_stats_query',
	'idx_traffic_stats_timeseries',
	'idx_protocol_stats_timeseries',
	'idx_address_count_stats_query',
	'idx_address_count_stats_timeseries',
	'idx_address_structure_stats_query',
	'idx_address_structure_stats_timeseries',
	'idx_port_count_stats_timeseries'
]) {
	db.exec(`DROP INDEX ${index}`);
}

db.exec(`
	CREATE TEMP TABLE perf_sources (source_id TEXT PRIMARY KEY) WITHOUT ROWID;
	WITH RECURSIVE sources(value) AS (
		VALUES (0)
		UNION ALL SELECT value + 1 FROM sources WHERE value + 1 < ${sourceCount}
	)
	INSERT INTO perf_sources SELECT printf('router-%02d', value) FROM sources;

	CREATE TEMP TABLE perf_buckets (bucket_start INTEGER PRIMARY KEY) WITHOUT ROWID;
	WITH RECURSIVE buckets(value) AS (
		VALUES (0)
		UNION ALL SELECT value + 1 FROM buckets WHERE value + 1 < ${fiveMinuteBucketCount}
	)
	INSERT INTO perf_buckets SELECT ${firstBucket} + value * 300 FROM buckets;

	CREATE TEMP TABLE perf_hour_buckets (bucket_start INTEGER PRIMARY KEY) WITHOUT ROWID;
	INSERT INTO perf_hour_buckets SELECT bucket_start FROM perf_buckets WHERE bucket_start % 3600 = 0;

	CREATE TEMP TABLE perf_day_buckets (bucket_start INTEGER PRIMARY KEY) WITHOUT ROWID;
	INSERT INTO perf_day_buckets SELECT bucket_start FROM perf_buckets WHERE bucket_start % 86400 = 28800;

	-- Keep the complete production scope shape in the 5m data. For the larger
	-- rollups, retain the all/all baseline and add one competing visibility
	-- scope only where the dashboard profiler looks so its visibility-leading
	-- indexes have a representative alternative without multiplying the whole
	-- fixture by ten.
	CREATE TEMP TABLE perf_scopes (
		ip_version INTEGER NOT NULL,
		src_visibility TEXT NOT NULL,
		dst_visibility TEXT NOT NULL,
		hourly_start INTEGER NOT NULL,
		hourly_end INTEGER NOT NULL,
		PRIMARY KEY(ip_version, src_visibility, dst_visibility)
	) WITHOUT ROWID;
	INSERT INTO perf_scopes (
		ip_version, src_visibility, dst_visibility, hourly_start, hourly_end
	) VALUES
		(4, 'all', 'all', ${firstBucket}, ${firstBucket + fiveMinuteBucketCount * 300}),
		(6, 'all', 'all', ${firstBucket}, ${firstBucket + fiveMinuteBucketCount * 300}),
		(4, 'literal', 'literal', ${profilerStartBucket}, ${profilerEndBucket}),
		(6, 'literal', 'literal', ${profilerStartBucket}, ${profilerEndBucket}),
		(4, 'literal', 'anonymized', 0, 0),
		(6, 'literal', 'anonymized', 0, 0),
		(4, 'anonymized', 'literal', 0, 0),
		(6, 'anonymized', 'literal', 0, 0),
		(4, 'anonymized', 'anonymized', 0, 0),
		(6, 'anonymized', 'anonymized', 0, 0);

	INSERT INTO datasets (
		id, label, default_start_date, source_mode, discovery_mode, sort_order
	) VALUES ('performance', 'Performance fixture', '2025-01-01', 'static', 'static', 0);

	INSERT INTO source_members (dataset_id, source_id, member_id)
	SELECT 'performance', source_id, source_id FROM perf_sources;

	INSERT INTO traffic_stats (
		source_id, granularity, bucket_start, bucket_end, ip_version,
		src_visibility, dst_visibility,
		flows, flows_tcp, flows_udp, flows_icmp, flows_other,
		packets, packets_tcp, packets_udp, packets_icmp, packets_other,
		bytes, bytes_tcp, bytes_udp, bytes_icmp, bytes_other,
		duration_sum_ms, duration_count, average_duration_ms,
		min_ttl_sum, min_ttl_count, average_min_ttl,
		max_ttl_sum, max_ttl_count, average_max_ttl
	)
	SELECT source_id, '5m', bucket_start, bucket_start + 300, ip_version,
		src_visibility, dst_visibility,
		100, 70, 20, 5, 5, 1000, 700, 200, 50, 50,
		100000, 70000, 20000, 5000, 5000,
		10000, 100, 100, 6400, 100, 64, 12800, 100, 128
	FROM perf_sources CROSS JOIN perf_buckets CROSS JOIN perf_scopes;

	INSERT INTO traffic_stats (
		source_id, granularity, bucket_start, bucket_end, ip_version,
		src_visibility, dst_visibility,
		flows, flows_tcp, flows_udp, flows_icmp, flows_other,
		packets, packets_tcp, packets_udp, packets_icmp, packets_other,
		bytes, bytes_tcp, bytes_udp, bytes_icmp, bytes_other,
		duration_sum_ms, duration_count, average_duration_ms,
		min_ttl_sum, min_ttl_count, average_min_ttl,
		max_ttl_sum, max_ttl_count, average_max_ttl
	)
	SELECT source_id, '1h', bucket_start, bucket_start + 3600, ip_version,
		src_visibility, dst_visibility, 12 * 100, 12 * 70, 12 * 20,
		12 * 5, 12 * 5, 12 * 1000, 12 * 700,
		12 * 200, 12 * 50, 12 * 50, 12 * 100000,
		12 * 70000, 12 * 20000, 12 * 5000, 12 * 5000,
		12 * 10000, 12 * 100, 100,
		12 * 6400, 12 * 100, 64,
		12 * 12800, 12 * 100, 128
	FROM perf_sources
	CROSS JOIN perf_hour_buckets
	CROSS JOIN perf_scopes
	WHERE bucket_start >= hourly_start AND bucket_start < hourly_end
	UNION ALL
	SELECT source_id, '1d', bucket_start, bucket_start + 86400, ip_version,
		'all', 'all', 288 * 100, 288 * 70, 288 * 20,
		288 * 5, 288 * 5, 288 * 1000, 288 * 700,
		288 * 200, 288 * 50, 288 * 50, 288 * 100000,
		288 * 70000, 288 * 20000, 288 * 5000, 288 * 5000,
		288 * 10000, 288 * 100, 100,
		288 * 6400, 288 * 100, 64,
		288 * 12800, 288 * 100, 128
	FROM perf_sources
	CROSS JOIN perf_day_buckets
	CROSS JOIN (SELECT 4 AS ip_version UNION ALL SELECT 6);

	INSERT INTO bucket_coverage (
		source_id, granularity, bucket_start, bucket_end,
		coverage_state, observed_units, expected_units, rejected_units
	)
	SELECT source_id, granularity, bucket_start, bucket_start + bucket_size,
		'complete', units, units, 0
	FROM perf_sources
	CROSS JOIN (
		SELECT '5m' AS granularity, bucket_start, 300 AS bucket_size, 1 AS units FROM perf_buckets
		UNION ALL
		SELECT '1h', bucket_start, 3600, 12 FROM perf_hour_buckets
		UNION ALL
		SELECT '1d', bucket_start, 86400, 288 FROM perf_day_buckets
	);

	INSERT INTO protocol_stats (
		source_id, granularity, bucket_start, bucket_end, ip_version,
		src_visibility, dst_visibility, unique_protocols_count, protocols_list
	)
	SELECT source_id, '1h', bucket_start, bucket_start + 3600, ip_version,
		src_visibility, dst_visibility, 4, '[6,17,1,58]'
	FROM perf_sources CROSS JOIN perf_hour_buckets CROSS JOIN perf_scopes
	WHERE bucket_start >= hourly_start AND bucket_start < hourly_end;

	INSERT INTO address_count_stats (
		source_id, granularity, bucket_start, bucket_end, ip_version,
		src_visibility, dst_visibility, address_side, unique_address_count
	)
	SELECT source_id, '1h', bucket_start, bucket_start + 3600, ip_version,
		src_visibility, dst_visibility, address_side, 1000
	FROM perf_sources CROSS JOIN perf_hour_buckets CROSS JOIN perf_scopes
	CROSS JOIN (SELECT 'source' AS address_side UNION ALL SELECT 'destination')
	WHERE bucket_start >= hourly_start AND bucket_start < hourly_end;

	INSERT INTO port_count_stats (
		source_id, granularity, bucket_start, bucket_end, ip_version,
		src_visibility, dst_visibility, port_side, port_range, unique_port_count
	)
	SELECT source_id, '1h', bucket_start, bucket_start + 3600, ip_version,
		src_visibility, dst_visibility, port_side, port_range, 128
	FROM perf_sources CROSS JOIN perf_hour_buckets CROSS JOIN perf_scopes
	CROSS JOIN (SELECT 'source' AS port_side UNION ALL SELECT 'destination')
	CROSS JOIN (SELECT 'low' AS port_range UNION ALL SELECT 'high')
	WHERE bucket_start >= hourly_start AND bucket_start < hourly_end;

	INSERT INTO address_structure_stats (
		source_id, granularity, bucket_start, bucket_end, ip_version,
		src_visibility, dst_visibility, address_side, structure_kind,
		values_json, metadata_json
	)
	SELECT source_id, '1h', bucket_start, bucket_start + 3600, 4,
		src_visibility, dst_visibility, address_side, structure_kind,
		CASE structure_kind
			WHEN 'spectrum' THEN '[{"alpha":0.1,"f":0.2},{"alpha":0.2,"f":0.3},{"alpha":0.3,"f":0.4}]'
			ELSE '[{"q":1,"tau":0.2,"sd":0.01},{"q":2,"tau":0.4,"sd":0.02}]'
		END,
		'{"uniqueAddressCount":1000}'
	FROM perf_sources CROSS JOIN perf_hour_buckets CROSS JOIN perf_scopes
	CROSS JOIN (SELECT 'source' AS address_side UNION ALL SELECT 'destination')
	CROSS JOIN (SELECT 'spectrum' AS structure_kind UNION ALL SELECT 'structure')
	WHERE ip_version = 4
		AND bucket_start >= hourly_start AND bucket_start < hourly_end;
`);

db.exec(localSchemaSql);
db.exec('ANALYZE; VACUUM;');

const tableCounts = Object.fromEntries(
	[
		'bucket_coverage',
		'traffic_stats',
		'protocol_stats',
		'address_count_stats',
		'port_count_stats',
		'address_structure_stats'
	].map((table) => [table, db.prepare(`SELECT COUNT(*) AS count FROM ${table}`).get()])
);
db.close();

console.log(
	JSON.stringify(
		{
			outputPath,
			sourceCount,
			fiveMinuteBucketCount,
			bytes: fs.statSync(outputPath).size,
			tableCounts
		},
		null,
		2
	)
);
