import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { discoverLocalSqlitePaths } from '../../../../src/lib/server/db/local-files';

describe('local SQLite discovery', () => {
	let dataDir: string;

	beforeEach(() => {
		dataDir = mkdtempSync(join(tmpdir(), 'atlantis-local-files-'));
		for (const dataset of ['beta', 'alpha']) {
			mkdirSync(join(dataDir, dataset));
			writeFileSync(join(dataDir, dataset, 'netflow.sqlite'), '');
		}
		mkdirSync(join(dataDir, 'backups'));
		writeFileSync(join(dataDir, 'backups', 'old.sqlite'), '');
	});

	afterEach(() => {
		vi.unstubAllEnvs();
		rmSync(dataDir, { recursive: true, force: true });
	});

	it('scans LOCAL_DATA_DIR for dataset products', async () => {
		vi.stubEnv('LOCAL_DATA_DIR', dataDir);

		await expect(discoverLocalSqlitePaths()).resolves.toEqual([
			join(dataDir, 'alpha', 'netflow.sqlite'),
			join(dataDir, 'beta', 'netflow.sqlite')
		]);
	});

	it('prefers LOCAL_SQLITE_PATH over LOCAL_DATA_DIR', async () => {
		vi.stubEnv('LOCAL_DATA_DIR', dataDir);
		vi.stubEnv('LOCAL_SQLITE_PATH', join(dataDir, 'beta', 'netflow.sqlite'));

		await expect(discoverLocalSqlitePaths()).resolves.toEqual([
			join(dataDir, 'beta', 'netflow.sqlite')
		]);
	});
});
