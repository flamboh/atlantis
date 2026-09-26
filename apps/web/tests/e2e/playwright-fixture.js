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
		);
		INSERT INTO bucket_coverage (
			source_id, granularity, bucket_start, bucket_end,
			coverage_state, observed_units, expected_units, rejected_units
		) VALUES ('fixture-router', '5m', 1740823200, 1740823500, 'complete', 1, 1, 0);
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
