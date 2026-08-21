import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import Database from 'better-sqlite3';
import { afterEach, describe, expect, it, vi } from 'vitest';

const ALERT_SCHEMA = `
	CREATE TABLE feed_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
	CREATE TABLE windows (
		window_start INTEGER PRIMARY KEY,
		window_end INTEGER NOT NULL,
		member_files INTEGER NOT NULL,
		address_count INTEGER NOT NULL,
		alert_count INTEGER NOT NULL,
		alpha_min REAL,
		alpha_max REAL,
		alpha_median REAL,
		processed_at INTEGER NOT NULL
	);
	CREATE TABLE alerts (
		window_start INTEGER NOT NULL REFERENCES windows(window_start) ON DELETE CASCADE,
		address TEXT NOT NULL,
		alpha REAL NOT NULL,
		tail TEXT NOT NULL CHECK (tail IN ('high', 'low')),
		rank INTEGER NOT NULL,
		r2 REAL NOT NULL,
		prefix_levels INTEGER NOT NULL,
		PRIMARY KEY (window_start, tail, rank)
	);
	CREATE INDEX alerts_address ON alerts(address, window_start);
`;

type Fixture = {
	directory: string;
	netflowPath: string;
	alertsPath: string;
};

async function loadAlertsModule() {
	vi.resetModules();
	return import('../../../src/lib/server/alerts');
}

function createDatasetFixture(datasetId = 'alpha'): Fixture {
	const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'alerts-test-'));
	const netflowPath = path.join(directory, 'netflow.sqlite');
	const db = new Database(netflowPath);
	db.exec(`
		CREATE TABLE datasets (
			id TEXT PRIMARY KEY NOT NULL,
			label TEXT NOT NULL,
			default_start_date TEXT NOT NULL,
			source_mode TEXT DEFAULT 'static' NOT NULL,
			discovery_mode TEXT DEFAULT 'static' NOT NULL,
			sort_order INTEGER DEFAULT 0 NOT NULL
		);
	`);
	db.prepare(
		`INSERT INTO datasets (
			id, label, default_start_date, source_mode, discovery_mode, sort_order
		) VALUES (?, ?, '2025-03-01', 'static', 'live', 0)`
	).run(datasetId, 'Alpha Label');
	db.close();

	return {
		directory,
		netflowPath,
		alertsPath: path.join(directory, 'alerts.sqlite')
	};
}

function seedAlertsDatabase(fixture: Fixture, windowCount = 2): void {
	const db = new Database(fixture.alertsPath);
	db.exec(ALERT_SCHEMA);
	const insertMeta = db.prepare('INSERT INTO feed_meta (key, value) VALUES (?, ?)');
	for (const [key, value] of [
		['schema_version', '1'],
		['dataset_id', 'alpha'],
		['threshold_high', '3.5'],
		['threshold_low', '0.4'],
		['max_per_tail', '25']
	] as const) {
		insertMeta.run(key, value);
	}

	const insertWindow = db.prepare(`
		INSERT INTO windows (
			window_start,
			window_end,
			member_files,
			address_count,
			alert_count,
			alpha_min,
			alpha_max,
			alpha_median,
			processed_at
		) VALUES (?, ?, 3, ?, ?, NULL, NULL, NULL, ?)
	`);
	const seedWindows = db.transaction(() => {
		for (let index = 0; index < windowCount; index += 1) {
			const windowStart = 1_700_000_000 + index * 300;
			const isLatestFixtureWindow = index === 1;
			insertWindow.run(
				windowStart,
				windowStart + 300,
				48_000 + index,
				isLatestFixtureWindow ? 3 : index === 0 ? 1 : 0,
				windowStart + 320
			);
		}
	});
	seedWindows();

	if (windowCount >= 1) {
		db.prepare(
			`INSERT INTO alerts (
				window_start, address, alpha, tail, rank, r2, prefix_levels
			) VALUES (?, '9.9.9.9', 0.21, 'low', 1, 0.91, 24)`
		).run(1_700_000_000);
	}
	if (windowCount >= 2) {
		const insertAlert = db.prepare(`
			INSERT INTO alerts (
				window_start, address, alpha, tail, rank, r2, prefix_levels
			) VALUES (?, ?, ?, ?, ?, ?, 24)
		`);
		const latestWindowStart = 1_700_000_300;
		insertAlert.run(latestWindowStart, '1.1.1.2', 3.7, 'high', 2, 0.94);
		insertAlert.run(latestWindowStart, '2.2.2.2', 0.2, 'low', 1, 0.89);
		insertAlert.run(latestWindowStart, '1.1.1.1', 3.9, 'high', 1, 0.98);
	}
	db.close();
}

describe('alerts server helper', () => {
	const originalCwd = process.cwd();

	afterEach(() => {
		process.chdir(originalCwd);
		vi.unstubAllEnvs();
	});

	it('returns feed metadata and severity-ordered address aggregates', async () => {
		const fixture = createDatasetFixture();
		seedAlertsDatabase(fixture);
		vi.stubEnv('LOCAL_SQLITE_PATH', fixture.netflowPath);
		const alerts = await loadAlertsModule();

		await expect(alerts.getAlertsFeedForDataset('alpha')).resolves.toEqual({
			feed: {
				present: true,
				latestWindowStart: 1_700_000_300,
				latestWindowEnd: 1_700_000_600,
				latestAddressCount: 48_001,
				latestProcessedAt: 1_700_000_620,
				thresholds: { high: 3.5, low: 0.4 }
			},
			horizonSeconds: 86_400,
			totalAddresses: 4,
			addresses: [
				{
					address: '1.1.1.1',
					tail: 'high',
					peakAlpha: 3.9,
					peakWindowStart: 1_700_000_300,
					peakR2: 0.98,
					lastSeen: 1_700_000_300,
					firstSeen: 1_700_000_300,
					timesFlagged: 1
				},
				{
					address: '1.1.1.2',
					tail: 'high',
					peakAlpha: 3.7,
					peakWindowStart: 1_700_000_300,
					peakR2: 0.94,
					lastSeen: 1_700_000_300,
					firstSeen: 1_700_000_300,
					timesFlagged: 1
				},
				{
					address: '2.2.2.2',
					tail: 'low',
					peakAlpha: 0.2,
					peakWindowStart: 1_700_000_300,
					peakR2: 0.89,
					lastSeen: 1_700_000_300,
					firstSeen: 1_700_000_300,
					timesFlagged: 1
				},
				{
					address: '9.9.9.9',
					tail: 'low',
					peakAlpha: 0.21,
					peakWindowStart: 1_700_000_000,
					peakR2: 0.91,
					lastSeen: 1_700_000_000,
					firstSeen: 1_700_000_000,
					timesFlagged: 1
				}
			]
		});
	});

	it('returns absent when a discovered dataset has no alerts database', async () => {
		const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'alerts-discovery-'));
		const datasetDirectory = path.join(workspace, 'data', 'alpha');
		fs.mkdirSync(datasetDirectory, { recursive: true });
		const fixture = createDatasetFixture();
		fs.renameSync(fixture.netflowPath, path.join(datasetDirectory, 'netflow.sqlite'));
		process.chdir(workspace);
		const alerts = await loadAlertsModule();

		await expect(alerts.getAlertsFeedForDataset('alpha')).resolves.toEqual({
			feed: { present: false },
			horizonSeconds: 86_400,
			totalAddresses: 0,
			addresses: []
		});
	});

	it('does not throw when a configured dataset database has no sibling alerts database', async () => {
		const fixture = createDatasetFixture();
		vi.stubEnv('LOCAL_SQLITE_PATH', fixture.netflowPath);
		const alerts = await loadAlertsModule();

		await expect(alerts.getAlertsFeedForDataset('alpha')).resolves.toEqual({
			feed: { present: false },
			horizonSeconds: 86_400,
			totalAddresses: 0,
			addresses: []
		});
	});

	it('opens a feed file that appears after an earlier absent result', async () => {
		const fixture = createDatasetFixture();
		vi.stubEnv('LOCAL_SQLITE_PATH', fixture.netflowPath);
		const alerts = await loadAlertsModule();

		await expect(alerts.getAlertsFeedForDataset('alpha')).resolves.toMatchObject({
			feed: { present: false }
		});
		seedAlertsDatabase(fixture);
		await expect(alerts.getAlertsFeedForDataset('alpha')).resolves.toMatchObject({
			feed: { present: true },
			totalAddresses: 4,
			addresses: expect.arrayContaining([
				expect.objectContaining({ address: '1.1.1.1' }),
				expect.objectContaining({ address: '9.9.9.9' })
			])
		});
	});

	it('evicts a cached feed handle when its file disappears', async () => {
		const fixture = createDatasetFixture();
		seedAlertsDatabase(fixture);
		vi.stubEnv('LOCAL_SQLITE_PATH', fixture.netflowPath);
		const alerts = await loadAlertsModule();

		await expect(alerts.getAlertsFeedForDataset('alpha')).resolves.toMatchObject({
			feed: { present: true }
		});
		fs.unlinkSync(fixture.alertsPath);
		await expect(alerts.getAlertsFeedForDataset('alpha')).resolves.toEqual({
			feed: { present: false },
			horizonSeconds: 86_400,
			totalAddresses: 0,
			addresses: []
		});
	});

	it('filters rows by tail before returning address aggregates', async () => {
		const fixture = createDatasetFixture();
		seedAlertsDatabase(fixture);
		vi.stubEnv('LOCAL_SQLITE_PATH', fixture.netflowPath);
		const alerts = await loadAlertsModule();

		const result = await alerts.getAlertsFeedForDataset('alpha', { tail: 'high' });
		expect(result.totalAddresses).toBe(2);
		expect(result.addresses).toMatchObject([
			{ address: '1.1.1.1', tail: 'high' },
			{ address: '1.1.1.2', tail: 'high' }
		]);
	});

	it('sorts recent addresses by last seen before severity', async () => {
		const fixture = createDatasetFixture();
		seedAlertsDatabase(fixture);
		vi.stubEnv('LOCAL_SQLITE_PATH', fixture.netflowPath);
		const alerts = await loadAlertsModule();

		const result = await alerts.getAlertsFeedForDataset('alpha', { sort: 'recent' });
		expect(result.addresses.map(({ address }) => address)).toEqual([
			'1.1.1.1',
			'1.1.1.2',
			'2.2.2.2',
			'9.9.9.9'
		]);
	});

	it('clamps address limits to the inclusive range from 1 through 500', async () => {
		const fixture = createDatasetFixture();
		seedAlertsDatabase(fixture, 600);
		const db = new Database(fixture.alertsPath);
		const insertAlert = db.prepare(`
			INSERT INTO alerts (
				window_start, address, alpha, tail, rank, r2, prefix_levels
			) VALUES (?, ?, 3.6, 'high', 1, 0.8, 24)
		`);
		for (let index = 2; index < 600; index += 1) {
			insertAlert.run(1_700_000_000 + index * 300, `address-${index}`);
		}
		db.close();
		vi.stubEnv('LOCAL_SQLITE_PATH', fixture.netflowPath);
		const alerts = await loadAlertsModule();

		const lower = await alerts.getAlertsFeedForDataset('alpha', { horizon: '7d', limit: 0 });
		const upper = await alerts.getAlertsFeedForDataset('alpha', { horizon: '7d', limit: 999 });
		expect(lower.addresses).toHaveLength(1);
		expect(upper.addresses).toHaveLength(500);
		expect(upper.totalAddresses).toBe(602);
	});
});
