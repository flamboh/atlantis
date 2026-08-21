import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import Database from 'better-sqlite3';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { getRequestedDataset } from '$lib/server/datasets';
import { GET } from '../../src/routes/api/alerts/+server';

vi.mock('$lib/server/datasets', () => ({
	getRequestedDataset: vi.fn()
}));

const LATEST_WINDOW_START = 200_000;
const LATEST_WINDOW_END = 200_300;
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

const fixtureDirectories: string[] = [];

function createFixture(withAlerts = true): Fixture {
	const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'api-alerts-test-'));
	fixtureDirectories.push(directory);
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
		) VALUES ('alpha', 'Alpha Label', '2025-03-01', 'static', 'live', 0)`
	).run();
	db.close();

	const fixture = {
		directory,
		netflowPath,
		alertsPath: path.join(directory, 'alerts.sqlite')
	};
	if (withAlerts) {
		seedAlerts(fixture);
	}
	return fixture;
}

function seedAlerts(fixture: Fixture): void {
	const db = new Database(fixture.alertsPath);
	db.exec(ALERT_SCHEMA);
	const insertMeta = db.prepare('INSERT INTO feed_meta (key, value) VALUES (?, ?)');
	for (const [key, value] of [
		['schema_version', '1'],
		['dataset_id', 'alpha'],
		['threshold_high', '2.0'],
		['threshold_low', '0.3'],
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
		) VALUES (?, ?, 1, 1000, 0, NULL, NULL, NULL, ?)
	`);
	for (const windowStart of [113_899, 196_600, 196_700, 199_700, LATEST_WINDOW_START]) {
		const windowEnd = windowStart === LATEST_WINDOW_START ? LATEST_WINDOW_END : windowStart + 300;
		insertWindow.run(windowStart, windowEnd, windowEnd + 20);
	}

	const insertAlert = db.prepare(`
		INSERT INTO alerts (
			window_start, address, alpha, tail, rank, r2, prefix_levels
		) VALUES (?, ?, ?, ?, ?, ?, 24)
	`);
	insertAlert.run(113_899, 'outside-24h', 12, 'high', 1, 0.5);
	// history for high-address outside every horizon: firstSeen must be
	// retention-wide (113_899) while in-horizon aggregates ignore this row.
	insertAlert.run(113_899, 'high-address', 2.05, 'high', 2, 0.5);
	insertAlert.run(196_600, 'old-high', 6, 'high', 1, 0.6);
	insertAlert.run(196_700, 'repeat', 2.5, 'high', 1, 0.7);
	insertAlert.run(199_700, 'repeat', 4.5, 'high', 1, 0.91);
	insertAlert.run(199_700, 'cross-tail', 2.1, 'high', 2, 0.65);
	insertAlert.run(199_700, 'low-address', 0, 'low', 1, 0.87);
	insertAlert.run(LATEST_WINDOW_START, 'repeat', 2.2, 'high', 1, 0.8);
	insertAlert.run(LATEST_WINDOW_START, 'high-address', 2.8, 'high', 2, 0.93);
	insertAlert.run(LATEST_WINDOW_START, 'cross-tail', 0.1, 'low', 1, 0.95);
	db.close();
}

function eventFor(query = '') {
	return {
		url: new URL(`http://localhost/api/alerts${query}`),
		platform: undefined
	} as never;
}

async function getJson(query = ''): Promise<{ response: Response; payload: unknown }> {
	const response = await GET(eventFor(query));
	return { response, payload: await response.json() };
}

describe('/api/alerts GET', () => {
	beforeEach(() => {
		vi.mocked(getRequestedDataset).mockReset().mockResolvedValue('alpha');
	});

	afterEach(() => {
		vi.unstubAllEnvs();
		for (const directory of fixtureDirectories.splice(0)) {
			fs.rmSync(directory, { recursive: true, force: true });
		}
	});

	it('deduplicates addresses and selects the most severe row inside the horizon', async () => {
		const fixture = createFixture();
		vi.stubEnv('LOCAL_SQLITE_PATH', fixture.netflowPath);

		const { response, payload } = await getJson('?dataset=alpha&horizon=1h');

		expect(response.status).toBe(200);
		expect(payload).toEqual({
			feed: {
				present: true,
				latestWindowStart: LATEST_WINDOW_START,
				latestWindowEnd: LATEST_WINDOW_END,
				latestAddressCount: 1000,
				latestProcessedAt: LATEST_WINDOW_END + 20,
				thresholds: { high: 2, low: 0.3 }
			},
			horizonSeconds: 3600,
			totalAddresses: 4,
			addresses: [
				{
					address: 'repeat',
					tail: 'high',
					peakAlpha: 4.5,
					peakWindowStart: 199_700,
					peakR2: 0.91,
					latestAlpha: 2.2,
					lastSeen: LATEST_WINDOW_START,
					firstSeen: 196_700,
					timesFlagged: 3
				},
				{
					address: 'high-address',
					tail: 'high',
					peakAlpha: 2.8,
					peakWindowStart: LATEST_WINDOW_START,
					peakR2: 0.93,
					latestAlpha: 2.8,
					lastSeen: LATEST_WINDOW_START,
					// retention-wide, not horizon-scoped: the 113_899 history row
					firstSeen: 113_899,
					timesFlagged: 1
				},
				{
					address: 'low-address',
					tail: 'low',
					peakAlpha: 0,
					peakWindowStart: 199_700,
					peakR2: 0.87,
					latestAlpha: 0,
					lastSeen: 199_700,
					firstSeen: 199_700,
					timesFlagged: 1
				},
				{
					address: 'cross-tail',
					tail: 'low',
					peakAlpha: 0.1,
					peakWindowStart: LATEST_WINDOW_START,
					peakR2: 0.95,
					latestAlpha: 0.1,
					lastSeen: LATEST_WINDOW_START,
					firstSeen: 199_700,
					timesFlagged: 2
				}
			]
		});
	});

	it('sorts by last seen and uses peak severity as the tiebreak', async () => {
		const fixture = createFixture();
		vi.stubEnv('LOCAL_SQLITE_PATH', fixture.netflowPath);

		const { payload } = await getJson('?sort=recent&horizon=1h');

		expect(payload).toMatchObject({ horizonSeconds: 3600, totalAddresses: 4 });
		expect(
			(payload as { addresses: Array<{ address: string }> }).addresses.map(({ address }) => address)
		).toEqual(['repeat', 'high-address', 'cross-tail', 'low-address']);
	});

	it('anchors horizon filtering on the latest processed window end', async () => {
		const fixture = createFixture();
		vi.stubEnv('LOCAL_SQLITE_PATH', fixture.netflowPath);

		const oneHour = await getJson('?horizon=1h');
		const oneDay = await getJson('?horizon=24h');

		expect((oneHour.payload as { addresses: Array<{ address: string }> }).addresses).toEqual(
			expect.arrayContaining([expect.objectContaining({ address: 'repeat', firstSeen: 196_700 })])
		);
		expect(
			(oneHour.payload as { addresses: Array<{ address: string }> }).addresses.some(
				({ address }) => address === 'old-high'
			)
		).toBe(false);
		expect(
			(oneDay.payload as { addresses: Array<{ address: string }> }).addresses.map(
				({ address }) => address
			)
		).toContain('old-high');
		expect(
			(oneDay.payload as { addresses: Array<{ address: string }> }).addresses.map(
				({ address }) => address
			)
		).not.toContain('outside-24h');
	});

	it('filters rows by tail before aggregating addresses', async () => {
		const fixture = createFixture();
		vi.stubEnv('LOCAL_SQLITE_PATH', fixture.netflowPath);

		const { payload } = await getJson('?tail=high&horizon=1h');
		const result = payload as {
			totalAddresses: number;
			addresses: Array<{ address: string; tail: string; timesFlagged: number; peakAlpha: number }>;
		};

		expect(result.totalAddresses).toBe(3);
		expect(result.addresses).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					address: 'cross-tail',
					tail: 'high',
					peakAlpha: 2.1,
					timesFlagged: 1
				})
			])
		);
		expect(result.addresses.every(({ tail }) => tail === 'high')).toBe(true);
	});

	it('applies the limit after counting all matching addresses', async () => {
		const fixture = createFixture();
		vi.stubEnv('LOCAL_SQLITE_PATH', fixture.netflowPath);

		const { payload } = await getJson('?horizon=24h&limit=2');
		const result = payload as { totalAddresses: number; addresses: unknown[] };

		expect(result.totalAddresses).toBe(5);
		expect(result.addresses).toHaveLength(2);
	});

	it.each([
		['?tail=middle', 'Invalid tail parameter'],
		['?horizon=2h', 'Invalid horizon parameter'],
		['?sort=alpha', 'Invalid sort parameter'],
		['?limit=many', 'Invalid limit parameter']
	])('rejects invalid query %s', async (query, message) => {
		const { response, payload } = await getJson(query);

		expect(response.status).toBe(400);
		expect(payload).toEqual({ data: null, error: message });
	});

	it('returns an absent feed as a normal response', async () => {
		const fixture = createFixture(false);
		vi.stubEnv('LOCAL_SQLITE_PATH', fixture.netflowPath);

		const { response, payload } = await getJson('?horizon=7d');

		expect(response.status).toBe(200);
		expect(payload).toEqual({
			feed: { present: false },
			horizonSeconds: 604_800,
			totalAddresses: 0,
			addresses: []
		});
	});
});
