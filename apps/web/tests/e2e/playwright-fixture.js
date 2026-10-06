import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import Database from 'better-sqlite3';
import { localSchemaSql } from '../../src/lib/server/db/local-schema';

export function seedPlaywrightDatabase() {
	const fixtureDirectory = fs.mkdtempSync(path.join(os.tmpdir(), 'atlantis-playwright-'));
	const databasePath = path.join(fixtureDirectory, 'netflow.sqlite');
	const database = new Database(databasePath);
	database.exec(localSchemaSql);
	const tau = `X'${'0000803F'.repeat(33)}'`;
	const tauSd = 'zeroblob(132)';
	database.exec(`
		INSERT INTO datasets (
			id, label, default_start_date, source_mode, discovery_mode, sort_order
		) VALUES ('playwright', 'Playwright Fixture', '2025-03-01', 'static', 'static', 0);
		INSERT INTO traffic_stats (
			source_id, granularity, bucket_start, bucket_end,
			ip_version, src_locality, dst_locality,
			flows, flows_tcp, flows_udp, flows_icmp, flows_other,
			packets, packets_tcp, packets_udp, packets_icmp, packets_other,
			bytes, bytes_tcp, bytes_udp, bytes_icmp, bytes_other,
			duration_sum_ms, duration_count, min_ttl_sum, min_ttl_count, max_ttl_sum, max_ttl_count
		) VALUES (
			'fixture-router', '5m', 1740823200, 1740823500, 4, 'all', 'all',
			0, 0, 0, 0, 0,
			0, 0, 0, 0, 0,
			0, 0, 0, 0, 0,
			0, 0, 0, 0, 0, 0
		), (
			'fixture-router', '1d', 1740816000, 1740902400, 4, 'all', 'all',
			10, 10, 0, 0, 0,
			0, 0, 0, 0, 0,
			0, 0, 0, 0, 0,
			0, 0, 0, 0, 0, 0
		);
		INSERT INTO address_count_stats (
			source_id, granularity, bucket_start, bucket_end,
			ip_version, src_locality, dst_locality, address_side, unique_address_count
		) VALUES
			('fixture-router', '1d', 1740816000, 1740902400, 4, 'all', 'all', 'source', 5),
			('fixture-router', '1d', 1740816000, 1740902400, 4, 'all', 'all', 'destination', 5);
		INSERT INTO bucket_coverage (
			source_id, granularity, bucket_start, bucket_end,
			coverage_state, observed_units, expected_units, rejected_units
		) VALUES
			('fixture-router', '5m', 1740823200, 1740823500, 'complete', 1, 1, 0),
			('fixture-router', '1d', 1740816000, 1740902400, 'complete', 288, 288, 0);
		INSERT INTO address_maad_stats (
			source_id, granularity, bucket_start, bucket_end, ip_version, src_locality, dst_locality,
			address_side, measure, total_addrs, d0, d1, d2, tau, tau_sd, spectrum
		) VALUES
			('fixture-router', '5m', 1740823200, 1740823500, 4, 'all', 'all', 'source', 'addresses',
				40, 0.92, 0.88, 0.85, ${tau}, ${tauSd}, X'6666663F0000803F0000C03F0000003F'),
			('fixture-router', '5m', 1740823200, 1740823500, 4, 'all', 'all', 'destination', 'addresses',
				40, 0.9, 0.86, 0.82, ${tau}, ${tauSd}, X'6666663F0000803F0000C03F0000003F'),
			('fixture-router', '5m', 1740823200, 1740823500, 4, 'all', 'all', 'source', 'packets',
				40, 0.92, 0.71, 0.63, ${tau}, ${tauSd}, NULL),
			('fixture-router', '5m', 1740823200, 1740823500, 4, 'all', 'all', 'destination', 'packets',
				40, 0.9, 0.69, 0.6, ${tau}, ${tauSd}, NULL),
			('fixture-router', '5m', 1740823200, 1740823500, 4, 'all', 'all', 'source', 'bytes',
				40, 0.92, 0.64, 0.55, ${tau}, ${tauSd}, NULL),
			('fixture-router', '5m', 1740823200, 1740823500, 4, 'all', 'all', 'destination', 'bytes',
				40, 0.9, 0.61, 0.52, ${tau}, ${tauSd}, NULL),
			('fixture-router', '1d', 1740816000, 1740902400, 4, 'all', 'all', 'source', 'packets',
				40, 0.92, 0.71, 0.63, ${tau}, ${tauSd}, NULL),
			('fixture-router', '1d', 1740816000, 1740902400, 4, 'all', 'all', 'destination', 'packets',
				40, 0.9, 0.69, 0.6, ${tau}, ${tauSd}, NULL);
		INSERT INTO maad_q_grid (ip_version, q_min, q_step, q_count) VALUES
			(4, -0.5, 0.125, 33),
			(6, -0.5, 0.125, 33);
	`);
	database.exec(`
		UPDATE datasets SET has_locality = 1 WHERE id = 'playwright';
		INSERT INTO datasets (
			id, label, default_start_date, source_mode, discovery_mode, sort_order, has_locality,
			maad_internal_side
		) VALUES (
			'playwright-external-maad', 'Playwright External MAAD', '2025-03-01', 'static', 'static',
			1, 1, 0
		);
		INSERT INTO address_maad_stats (
			source_id, granularity, bucket_start, bucket_end, ip_version, src_locality, dst_locality,
			address_side, measure, total_addrs, d0, d1, d2, tau, tau_sd, spectrum
		)
		SELECT source_id, g.granularity,
			CASE WHEN g.granularity = '1d' THEN 1740816000 ELSE 1740823200 END,
			CASE WHEN g.granularity = '1d' THEN 1740902400 ELSE 1740823200 + g.duration END,
			ip_version, src_locality, dst_locality, address_side, measure, total_addrs,
			d0, d1, d2, tau, tau_sd, spectrum
		FROM address_maad_stats CROSS JOIN (
			SELECT '1d' AS granularity, 86400 AS duration UNION ALL SELECT '1h', 3600
			UNION ALL SELECT '30m', 1800 UNION ALL SELECT '10m', 600
		) g
		WHERE address_maad_stats.granularity = '5m' AND measure = 'addresses';
		INSERT INTO address_maad_stats (
			source_id, granularity, bucket_start, bucket_end, ip_version, src_locality, dst_locality,
			address_side, measure, total_addrs, d0, d1, d2, tau, tau_sd, spectrum
		)
		SELECT source_id, granularity, bucket_start, bucket_end, ip_version, l.src, l.dst,
			address_side, measure, total_addrs, d0, d1, d2, tau, tau_sd, spectrum
		FROM address_maad_stats CROSS JOIN (
			SELECT 'external' AS src, 'internal' AS dst UNION ALL SELECT 'internal', 'external'
			UNION ALL SELECT 'internal', 'internal' UNION ALL SELECT 'external', 'external'
		) l WHERE src_locality = 'all' AND measure = 'addresses';
	`);

	database.exec(`
		INSERT INTO bucket_coverage (
			source_id, granularity, bucket_start, bucket_end,
			coverage_state, observed_units, expected_units, rejected_units
		)
		SELECT DISTINCT source_id, granularity, bucket_start, bucket_end,
			'complete', (bucket_end - bucket_start) / 300, (bucket_end - bucket_start) / 300, 0
		FROM address_maad_stats WHERE granularity IN ('1h', '30m', '10m');
	`);

	database.exec(`
		UPDATE traffic_stats SET
			flows = 120, flows_tcp = 80, flows_udp = 30, flows_icmp = 8, flows_other = 2,
			packets = 1200, packets_tcp = 800, packets_udp = 300, packets_icmp = 80, packets_other = 20,
			bytes = 122880, bytes_tcp = 81920, bytes_udp = 30720, bytes_icmp = 8192, bytes_other = 2048,
			duration_sum_ms = 240000, duration_count = 120,
			min_ttl_sum = 3840, min_ttl_count = 120, max_ttl_sum = 7680, max_ttl_count = 120;
		INSERT INTO traffic_stats (
			source_id, granularity, bucket_start, bucket_end, ip_version, src_locality, dst_locality,
			flows, flows_tcp, flows_udp, flows_icmp, flows_other,
			packets, packets_tcp, packets_udp, packets_icmp, packets_other,
			bytes, bytes_tcp, bytes_udp, bytes_icmp, bytes_other,
			duration_sum_ms, duration_count, min_ttl_sum, min_ttl_count, max_ttl_sum, max_ttl_count
		)
		SELECT source_id, granularity, bucket_start, bucket_end, 6, src_locality, dst_locality,
			40, 20, 10, 8, 2, 400, 200, 100, 80, 20, 40960, 20480, 10240, 8192, 2048,
			40000, 40, 640, 40, 1280, 40 FROM traffic_stats;
		INSERT INTO traffic_stats (
			source_id, granularity, bucket_start, bucket_end, ip_version, src_locality, dst_locality,
			flows, flows_tcp, flows_udp, flows_icmp, flows_other,
			packets, packets_tcp, packets_udp, packets_icmp, packets_other,
			bytes, bytes_tcp, bytes_udp, bytes_icmp, bytes_other,
			duration_sum_ms, duration_count, min_ttl_sum, min_ttl_count, max_ttl_sum, max_ttl_count
		)
		SELECT t.source_id, g.granularity, 1740823200, 1740823200 + g.duration,
			t.ip_version, t.src_locality, t.dst_locality,
			t.flows, t.flows_tcp, t.flows_udp, t.flows_icmp, t.flows_other,
			t.packets, t.packets_tcp, t.packets_udp, t.packets_icmp, t.packets_other,
			t.bytes, t.bytes_tcp, t.bytes_udp, t.bytes_icmp, t.bytes_other,
			t.duration_sum_ms, t.duration_count, t.min_ttl_sum, t.min_ttl_count,
			t.max_ttl_sum, t.max_ttl_count
		FROM traffic_stats t CROSS JOIN (
			SELECT '1h' AS granularity, 3600 AS duration UNION ALL SELECT '30m', 1800
			UNION ALL SELECT '10m', 600
		) g WHERE t.granularity = '5m';
		INSERT INTO traffic_stats (
			source_id, granularity, bucket_start, bucket_end, ip_version, src_locality, dst_locality,
			flows, flows_tcp, flows_udp, flows_icmp, flows_other,
			packets, packets_tcp, packets_udp, packets_icmp, packets_other,
			bytes, bytes_tcp, bytes_udp, bytes_icmp, bytes_other,
			duration_sum_ms, duration_count, min_ttl_sum, min_ttl_count, max_ttl_sum, max_ttl_count
		)
		SELECT t.source_id, t.granularity, t.bucket_start, t.bucket_end, t.ip_version, 'external', 'internal',
			t.flows / 2, t.flows_tcp / 2, t.flows_udp / 2, t.flows_icmp / 2, t.flows_other / 2,
			t.packets / 2, t.packets_tcp / 2, t.packets_udp / 2, t.packets_icmp / 2, t.packets_other / 2,
			t.bytes / 2, t.bytes_tcp / 2, t.bytes_udp / 2, t.bytes_icmp / 2, t.bytes_other / 2,
			t.duration_sum_ms / 2, t.duration_count / 2, t.min_ttl_sum / 2, t.min_ttl_count / 2,
			t.max_ttl_sum / 2, t.max_ttl_count / 2 FROM traffic_stats t;
		INSERT INTO address_count_stats (
			source_id, granularity, bucket_start, bucket_end, ip_version, src_locality, dst_locality,
			address_side, unique_address_count
		)
		SELECT DISTINCT source_id, granularity, bucket_start, bucket_end, ip_version, 'all', 'all',
			s.side, CASE WHEN ip_version = 4 THEN 5 ELSE 3 END
		FROM traffic_stats CROSS JOIN (SELECT 'source' AS side UNION ALL SELECT 'destination') s
		WHERE src_locality = 'all' AND granularity <> '1d';
		INSERT INTO protocol_stats (
			source_id, granularity, bucket_start, bucket_end, ip_version, src_locality, dst_locality,
			unique_protocols_count, protocols_list
		)
		SELECT source_id, granularity, bucket_start, bucket_end, ip_version, 'all', 'all',
			CASE WHEN ip_version = 4 THEN 3 ELSE 2 END, '[1,6,17]'
		FROM traffic_stats WHERE src_locality = 'all';
		INSERT INTO port_count_stats (
			source_id, granularity, bucket_start, bucket_end, ip_version, src_locality, dst_locality,
			port_side, port_range, unique_port_count
		)
		SELECT source_id, granularity, bucket_start, bucket_end, ip_version, 'all', 'all',
			s.side, r.range, CASE WHEN ip_version = 4 THEN 8 ELSE 2 END
		FROM traffic_stats CROSS JOIN (SELECT 'source' AS side UNION ALL SELECT 'destination') s
		CROSS JOIN (SELECT 'low' AS range UNION ALL SELECT 'high') r WHERE src_locality = 'all';
	`);

	database.exec(`
		INSERT INTO traffic_stats (
			source_id, granularity, bucket_start, bucket_end, ip_version, src_locality, dst_locality,
			flows, flows_tcp, flows_udp, flows_icmp, flows_other,
			packets, packets_tcp, packets_udp, packets_icmp, packets_other,
			bytes, bytes_tcp, bytes_udp, bytes_icmp, bytes_other,
			duration_sum_ms, duration_count, min_ttl_sum, min_ttl_count, max_ttl_sum, max_ttl_count
		)
		SELECT source_id, granularity, bucket_end, bucket_end + bucket_end - bucket_start,
			ip_version, src_locality, dst_locality,
			1, 0, 1, 0, 0, 1, 0, 1, 0, 0, 1024, 0, 1024, 0, 0,
			CASE WHEN ip_version = 4 THEN 2000 ELSE 1000 END, 1,
			CASE WHEN ip_version = 4 THEN 32 ELSE 16 END, 1,
			CASE WHEN ip_version = 4 THEN 64 ELSE 32 END, 1
		FROM traffic_stats WHERE granularity <> '1d';
		UPDATE traffic_stats SET flows = flows + 1, flows_udp = flows_udp + 1,
			packets = packets + 1, packets_udp = packets_udp + 1,
			bytes = bytes + 1024, bytes_udp = bytes_udp + 1024,
			duration_sum_ms = duration_sum_ms + CASE WHEN ip_version = 4 THEN 2000 ELSE 1000 END,
			duration_count = duration_count + 1,
			min_ttl_sum = min_ttl_sum + CASE WHEN ip_version = 4 THEN 32 ELSE 16 END,
			min_ttl_count = min_ttl_count + 1,
			max_ttl_sum = max_ttl_sum + CASE WHEN ip_version = 4 THEN 64 ELSE 32 END,
			max_ttl_count = max_ttl_count + 1
		WHERE granularity = '1d';
		INSERT INTO bucket_coverage
		SELECT source_id, granularity, bucket_end, bucket_end + bucket_end - bucket_start,
			coverage_state, observed_units, expected_units, rejected_units
		FROM bucket_coverage WHERE granularity <> '1d';
		INSERT INTO port_count_stats (
			source_id, granularity, bucket_start, bucket_end, ip_version, src_locality, dst_locality,
			port_side, port_range, unique_port_count
		)
		SELECT source_id, granularity, bucket_end, bucket_end + bucket_end - bucket_start,
			ip_version, src_locality, dst_locality, port_side, port_range, unique_port_count
		FROM port_count_stats WHERE granularity <> '1d';
	`);

	database.exec(`
		INSERT INTO traffic_stats (
			source_id, granularity, bucket_start, bucket_end, ip_version, src_locality, dst_locality,
			flows, flows_tcp, flows_udp, flows_icmp, flows_other,
			packets, packets_tcp, packets_udp, packets_icmp, packets_other,
			bytes, bytes_tcp, bytes_udp, bytes_icmp, bytes_other,
			duration_sum_ms, duration_count, min_ttl_sum, min_ttl_count, max_ttl_sum, max_ttl_count
		)
		SELECT source_id, granularity, bucket_start + 172800, bucket_end + 172800,
			ip_version, src_locality, dst_locality,
			flows * 2, flows_tcp * 2, flows_udp * 2, flows_icmp * 2, flows_other * 2,
			packets * 2, packets_tcp * 2, packets_udp * 2, packets_icmp * 2, packets_other * 2,
			bytes * 2, bytes_tcp * 2, bytes_udp * 2, bytes_icmp * 2, bytes_other * 2,
			duration_sum_ms * 2, duration_count * 2, min_ttl_sum * 2, min_ttl_count * 2,
			max_ttl_sum * 2, max_ttl_count * 2 FROM traffic_stats;
		INSERT INTO bucket_coverage
		SELECT source_id, granularity, bucket_start + 172800, bucket_end + 172800,
			coverage_state, observed_units, expected_units, rejected_units FROM bucket_coverage;
	`);

	database.close();
	const alerts = new Database(path.join(fixtureDirectory, 'alerts.sqlite'));
	alerts.exec(`
		CREATE TABLE feed_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
		INSERT INTO feed_meta VALUES ('schema_version', '1'), ('dataset_id', 'playwright'),
			('threshold_high', '1.5'), ('threshold_low', '0.5'), ('max_per_tail', '200');
		CREATE TABLE windows (
			window_start INTEGER PRIMARY KEY, window_end INTEGER, member_files INTEGER,
			address_count INTEGER, alert_count INTEGER, alpha_min REAL, alpha_max REAL,
			alpha_median REAL, processed_at INTEGER
		);
		CREATE TABLE alerts (
			window_start INTEGER, address TEXT, alpha REAL, tail TEXT, rank INTEGER,
			r2 REAL, prefix_levels INTEGER, PRIMARY KEY (window_start, address)
		);
	`);
	const now = Math.floor(Date.now() / 1000);
	const start = now - 300;
	alerts
		.prepare('INSERT INTO windows VALUES (?, ?, 1, 1000, 106, 0.1, 3, 1, ?)')
		.run(start, now, now);
	const insertAlert = alerts.prepare('INSERT INTO alerts VALUES (?, ?, ?, ?, ?, 0.98, 17)');
	alerts.transaction(() => {
		for (let index = 1; index <= 105; index += 1) {
			insertAlert.run(
				index === 105 ? now - 7200 : start,
				`198.51.100.${index}`,
				2 + index / 1000,
				'high',
				index
			);
		}
		insertAlert.run(now - 7200, '203.0.113.7', 0.2, 'low', 1);
	})();
	alerts.close();
	return { databasePath, fixtureDirectory };
}

/** @param {string} fixtureDirectory */
export function cleanupPlaywrightDatabase(fixtureDirectory) {
	const resolvedDirectory = path.resolve(fixtureDirectory);
	if (
		path.dirname(resolvedDirectory) !== path.resolve(os.tmpdir()) ||
		!path.basename(resolvedDirectory).startsWith('atlantis-playwright-')
	) {
		throw new Error(`Refusing to remove unexpected Playwright fixture path: ${fixtureDirectory}`);
	}
	fs.rmSync(resolvedDirectory, { recursive: true, force: true });
}
