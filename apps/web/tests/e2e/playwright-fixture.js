import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import Database from 'better-sqlite3';

export function seedPlaywrightDatabase() {
	const fixtureDirectory = fs.mkdtempSync(path.join(os.tmpdir(), 'atlantis-playwright-'));
	const databasePath = path.join(fixtureDirectory, 'netflow.sqlite');
	const database = new Database(databasePath);
	database.exec(`
		CREATE TABLE datasets (
			id TEXT PRIMARY KEY NOT NULL,
			label TEXT NOT NULL,
			default_start_date TEXT NOT NULL,
			source_mode TEXT DEFAULT 'static' NOT NULL,
			discovery_mode TEXT DEFAULT 'static' NOT NULL,
			sort_order INTEGER DEFAULT 0 NOT NULL
		);
		CREATE TABLE traffic_stats (
			source_id TEXT NOT NULL,
			granularity TEXT NOT NULL,
			bucket_start INTEGER NOT NULL,
			bucket_end INTEGER NOT NULL,
			ip_version INTEGER NOT NULL,
			src_visibility TEXT NOT NULL,
			dst_visibility TEXT NOT NULL,
			flows INTEGER NOT NULL DEFAULT 0,
			flows_tcp INTEGER NOT NULL DEFAULT 0,
			flows_udp INTEGER NOT NULL DEFAULT 0,
			flows_icmp INTEGER NOT NULL DEFAULT 0,
			flows_other INTEGER NOT NULL DEFAULT 0,
			packets INTEGER NOT NULL DEFAULT 0,
			packets_tcp INTEGER NOT NULL DEFAULT 0,
			packets_udp INTEGER NOT NULL DEFAULT 0,
			packets_icmp INTEGER NOT NULL DEFAULT 0,
			packets_other INTEGER NOT NULL DEFAULT 0,
			bytes INTEGER NOT NULL DEFAULT 0,
			bytes_tcp INTEGER NOT NULL DEFAULT 0,
			bytes_udp INTEGER NOT NULL DEFAULT 0,
			bytes_icmp INTEGER NOT NULL DEFAULT 0,
			bytes_other INTEGER NOT NULL DEFAULT 0,
			duration_sum_ms INTEGER NOT NULL DEFAULT 0,
			duration_count INTEGER NOT NULL DEFAULT 0,
			average_duration_ms REAL,
			min_ttl_sum INTEGER NOT NULL DEFAULT 0,
			min_ttl_count INTEGER NOT NULL DEFAULT 0,
			average_min_ttl REAL,
			max_ttl_sum INTEGER NOT NULL DEFAULT 0,
			max_ttl_count INTEGER NOT NULL DEFAULT 0,
			average_max_ttl REAL,
			processed_at TEXT DEFAULT CURRENT_TIMESTAMP
		);
		CREATE TABLE protocol_stats (
			source_id TEXT NOT NULL,
			granularity TEXT NOT NULL,
			bucket_start INTEGER NOT NULL,
			bucket_end INTEGER NOT NULL,
			ip_version INTEGER NOT NULL,
			src_visibility TEXT NOT NULL,
			dst_visibility TEXT NOT NULL,
			unique_protocols_count INTEGER NOT NULL,
			protocols_list TEXT NOT NULL,
			processed_at TEXT DEFAULT CURRENT_TIMESTAMP
		);
		CREATE TABLE address_count_stats (
			source_id TEXT NOT NULL,
			granularity TEXT NOT NULL,
			bucket_start INTEGER NOT NULL,
			bucket_end INTEGER NOT NULL,
			ip_version INTEGER NOT NULL,
			src_visibility TEXT NOT NULL,
			dst_visibility TEXT NOT NULL,
			address_side TEXT NOT NULL,
			unique_address_count INTEGER NOT NULL,
			processed_at TEXT DEFAULT CURRENT_TIMESTAMP
		);
		CREATE TABLE address_structure_stats (
			source_id TEXT NOT NULL,
			granularity TEXT NOT NULL,
			bucket_start INTEGER NOT NULL,
			bucket_end INTEGER NOT NULL,
			ip_version INTEGER NOT NULL,
			src_visibility TEXT NOT NULL,
			dst_visibility TEXT NOT NULL,
			address_side TEXT NOT NULL,
			structure_kind TEXT NOT NULL,
			values_json TEXT NOT NULL,
			metadata_json TEXT NOT NULL,
			processed_at TEXT DEFAULT CURRENT_TIMESTAMP
		);
		CREATE TABLE port_count_stats (
			source_id TEXT NOT NULL,
			granularity TEXT NOT NULL,
			bucket_start INTEGER NOT NULL,
			bucket_end INTEGER NOT NULL,
			ip_version INTEGER NOT NULL,
			src_visibility TEXT NOT NULL,
			dst_visibility TEXT NOT NULL,
			port_side TEXT NOT NULL,
			port_range TEXT NOT NULL,
			unique_port_count INTEGER NOT NULL,
			processed_at TEXT DEFAULT CURRENT_TIMESTAMP
		);
		CREATE TABLE bucket_coverage (
			source_id TEXT NOT NULL,
			granularity TEXT NOT NULL,
			bucket_start INTEGER NOT NULL,
			bucket_end INTEGER NOT NULL,
			coverage_state TEXT NOT NULL,
			observed_units INTEGER NOT NULL,
			expected_units INTEGER NOT NULL,
			rejected_units INTEGER NOT NULL,
			PRIMARY KEY (source_id, granularity, bucket_start)
		);
		INSERT INTO datasets (
			id, label, default_start_date, source_mode, discovery_mode, sort_order
		) VALUES ('playwright', 'Playwright Fixture', '2025-03-01', 'static', 'static', 0);
		INSERT INTO traffic_stats (
			source_id, granularity, bucket_start, bucket_end,
			ip_version, src_visibility, dst_visibility
		) VALUES ('fixture-router', '5m', 1740823200, 1740823500, 4, 'all', 'all');
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
