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
			dst_visibility TEXT NOT NULL
		);
		INSERT INTO datasets (
			id, label, default_start_date, source_mode, discovery_mode, sort_order
		) VALUES ('playwright', 'Playwright Fixture', '2025-03-01', 'static', 'static', 0);
		INSERT INTO traffic_stats (
			source_id, granularity, bucket_start, bucket_end,
			ip_version, src_visibility, dst_visibility
		) VALUES ('fixture-router', '5m', 1740823200, 1740823500, 4, 'all', 'all');
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
