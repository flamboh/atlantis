import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import Database from 'better-sqlite3';
import { describe, expect, it } from 'vitest';

const migrationsDirectory = fileURLToPath(new URL('../../../drizzle', import.meta.url));

describe('D1 migrations', () => {
	it('bootstrap the canonical observation schema from an empty database', () => {
		const database = new Database(':memory:');
		const migrations = fs
			.readdirSync(migrationsDirectory)
			.filter((fileName) => fileName.endsWith('.sql'))
			.sort();

		try {
			for (const migration of migrations) {
				database.exec(fs.readFileSync(path.join(migrationsDirectory, migration), 'utf8'));
			}

			const trafficColumns = database
				.prepare('PRAGMA table_info(traffic_stats)')
				.all()
				.map((column) => (column as { name: string }).name);
			expect(trafficColumns).toEqual(
				expect.arrayContaining([
					'duration_sum_ms',
					'duration_count',
					'average_duration_ms',
					'min_ttl_sum',
					'min_ttl_count',
					'average_min_ttl',
					'max_ttl_sum',
					'max_ttl_count',
					'average_max_ttl'
				])
			);

			const portColumns = database
				.prepare('PRAGMA table_info(port_count_stats)')
				.all()
				.map((column) => (column as { name: string }).name);
			expect(portColumns).toEqual(
				expect.arrayContaining(['port_side', 'port_range', 'unique_port_count'])
			);
		} finally {
			database.close();
		}
	});
});
