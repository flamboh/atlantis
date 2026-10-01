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

	database.close();
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
