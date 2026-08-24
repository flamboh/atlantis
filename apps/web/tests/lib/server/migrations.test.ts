import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import Database from 'better-sqlite3';
import { describe, expect, it } from 'vitest';

const migrationsDirectory = fileURLToPath(new URL('../../../drizzle', import.meta.url));

function indexColumns(database: Database.Database, indexName: string): string[] {
	return database
		.prepare(`PRAGMA index_info('${indexName}')`)
		.all()
		.map((column) => (column as { name: string }).name);
}

function migrationFiles(): string[] {
	return fs
		.readdirSync(migrationsDirectory)
		.filter((fileName) => fileName.endsWith('.sql'))
		.sort();
}

function applyMigration(database: Database.Database, fileName: string): void {
	database.exec(fs.readFileSync(path.join(migrationsDirectory, fileName), 'utf8'));
}

function seedPlannerStatistics(database: Database.Database): void {
	const insertTraffic = database.prepare(`
		INSERT INTO traffic_stats (
			source_id, granularity, bucket_start, bucket_end, ip_version,
			src_visibility, dst_visibility, flows, flows_tcp, flows_udp,
			flows_icmp, flows_other, packets, packets_tcp, packets_udp,
			packets_icmp, packets_other, bytes, bytes_tcp, bytes_udp,
			bytes_icmp, bytes_other, duration_sum_ms, duration_count,
			average_duration_ms, min_ttl_sum, min_ttl_count, average_min_ttl,
			max_ttl_sum, max_ttl_count, average_max_ttl
		) VALUES (
			?, '5m', ?, ?, 4, 'public', 'private', 1, 1, 0, 0, 0,
			1, 1, 0, 0, 0, 100, 100, 0, 0, 0, 10, 1, 10, 64, 1, 64, 64, 1, 64
		)
	`);
	const insertProtocol = database.prepare(`
		INSERT INTO protocol_stats (
			source_id, granularity, bucket_start, bucket_end, ip_version,
			src_visibility, dst_visibility, unique_protocols_count, protocols_list
		) VALUES (?, '5m', ?, ?, 4, 'public', 'private', 1, 'tcp')
	`);
	const insertPort = database.prepare(`
		INSERT INTO port_count_stats (
			source_id, granularity, bucket_start, bucket_end, ip_version,
			src_visibility, dst_visibility, port_side, port_range, unique_port_count
		) VALUES (?, '5m', ?, ?, 4, 'public', 'private', 'src', 'well-known', 1)
	`);
	const insertAddress = database.prepare(`
		INSERT INTO address_count_stats (
			source_id, granularity, bucket_start, bucket_end, ip_version,
			src_visibility, dst_visibility, address_side, unique_address_count
		) VALUES (?, '5m', ?, ?, 4, 'public', 'private', 'src', 1)
	`);
	const insertStructure = database.prepare(`
		INSERT INTO address_structure_stats (
			source_id, granularity, bucket_start, bucket_end, ip_version,
			src_visibility, dst_visibility, address_side, structure_kind,
			values_json, metadata_json
		) VALUES (?, '5m', ?, ?, 4, 'public', 'private', 'src', 'prefix', '[]', '{}')
	`);

	database.transaction(() => {
		for (const bucketStart of [0, 300, 600, 900]) {
			const sourceId = `stats-source-${bucketStart / 300}`;
			const bucketEnd = bucketStart + 300;
			insertTraffic.run(sourceId, bucketStart, bucketEnd);
			insertProtocol.run(sourceId, bucketStart, bucketEnd);
			insertPort.run(sourceId, bucketStart, bucketEnd);
			insertAddress.run(sourceId, bucketStart, bucketEnd);
			insertStructure.run(sourceId, bucketStart, bucketEnd);
		}
	})();
}

describe('D1 migrations', () => {
	it('bootstrap the canonical observation schema from an empty database', () => {
		const database = new Database(':memory:');
		const migrations = migrationFiles();

		try {
			for (const migration of migrations) {
				applyMigration(database, migration);
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

			const coverageColumns = database
				.prepare('PRAGMA table_info(bucket_coverage)')
				.all()
				.map((column) => (column as { name: string }).name);
			expect(coverageColumns).toEqual(
				expect.arrayContaining([
					'coverage_state',
					'observed_units',
					'expected_units',
					'rejected_units'
				])
			);
			expect(() =>
				database
					.prepare(
						`INSERT INTO bucket_coverage (
							source_id, granularity, bucket_start, bucket_end, coverage_state,
							observed_units, expected_units, rejected_units
						) VALUES ('r1', '5m', 0, 300, 'complete', 0, 1, 0)`
					)
					.run()
			).toThrow();

			const expectedTimeseriesIndexes = new Map([
				[
					'idx_traffic_stats_timeseries',
					['source_id', 'granularity', 'src_visibility', 'dst_visibility', 'bucket_start']
				],
				[
					'idx_protocol_stats_timeseries',
					['source_id', 'granularity', 'src_visibility', 'dst_visibility', 'bucket_start']
				],
				[
					'idx_address_count_stats_timeseries',
					['source_id', 'granularity', 'src_visibility', 'dst_visibility', 'bucket_start']
				],
				[
					'idx_port_count_stats_timeseries',
					['source_id', 'granularity', 'src_visibility', 'dst_visibility', 'bucket_start']
				],
				[
					'idx_address_structure_stats_timeseries',
					[
						'source_id',
						'granularity',
						'src_visibility',
						'dst_visibility',
						'ip_version',
						'structure_kind',
						'bucket_start'
					]
				]
			]);
			for (const [indexName, columns] of expectedTimeseriesIndexes) {
				expect(indexColumns(database, indexName), indexName).toEqual(columns);
			}

			const coveragePrimaryKey = database
				.prepare("PRAGMA table_info('bucket_coverage')")
				.all()
				.map((column) => column as { name: string; pk: number })
				.filter((column) => column.pk > 0)
				.sort((left, right) => left.pk - right.pk)
				.map((column) => column.name);
			expect(coveragePrimaryKey).toEqual(['source_id', 'granularity', 'bucket_start']);

			for (const indexName of [
				'idx_traffic_stats_query',
				'idx_address_count_stats_query',
				'idx_address_structure_stats_query'
			]) {
				expect(indexColumns(database, indexName), indexName).not.toHaveLength(0);
			}
			expect(indexColumns(database, 'idx_protocol_stats_query')).toHaveLength(0);
			expect(indexColumns(database, 'idx_port_count_stats_query')).toHaveLength(0);
		} finally {
			database.close();
		}
	});

	it('upgrades the deployed coverage schema without retaining unused bucket-first indexes', () => {
		const database = new Database(':memory:');
		const migrations = migrationFiles();
		try {
			for (const migration of migrations.slice(0, 2)) applyMigration(database, migration);
			expect(indexColumns(database, 'idx_protocol_stats_query')).not.toHaveLength(0);
			expect(indexColumns(database, 'idx_port_count_stats_query')).not.toHaveLength(0);

			for (const migration of migrations.slice(2)) applyMigration(database, migration);
			expect(indexColumns(database, 'idx_protocol_stats_query')).toHaveLength(0);
			expect(indexColumns(database, 'idx_port_count_stats_query')).toHaveLength(0);
			expect(indexColumns(database, 'idx_protocol_stats_timeseries')).not.toHaveLength(0);
			expect(indexColumns(database, 'idx_port_count_stats_timeseries')).not.toHaveLength(0);
		} finally {
			database.close();
		}
	});

	it('refreshes planner statistics for the replacement timeseries indexes', () => {
		const database = new Database(':memory:');
		const migrations = migrationFiles();
		const timeseriesIndexes = [
			'idx_traffic_stats_timeseries',
			'idx_protocol_stats_timeseries',
			'idx_address_count_stats_timeseries',
			'idx_port_count_stats_timeseries',
			'idx_address_structure_stats_timeseries'
		];

		try {
			for (const migration of migrations.slice(0, 2)) applyMigration(database, migration);
			seedPlannerStatistics(database);
			database.exec('ANALYZE');

			const oldStatistics = database
				.prepare(
					`SELECT idx FROM sqlite_stat1
					 WHERE idx IN (
						'idx_traffic_stats_query',
						'idx_protocol_stats_query',
						'idx_address_count_stats_query',
						'idx_port_count_stats_query',
						'idx_address_structure_stats_query'
					 )`
				)
				.all() as Array<{ idx: string }>;
			expect(oldStatistics).toHaveLength(5);

			applyMigration(database, migrations[2]);

			const refreshedStatistics = database
				.prepare(
					`SELECT idx, stat FROM sqlite_stat1
					 WHERE idx IN (${timeseriesIndexes.map(() => '?').join(', ')})`
				)
				.all(...timeseriesIndexes) as Array<{ idx: string; stat: string }>;
			const statisticsByIndex = new Map(refreshedStatistics.map(({ idx, stat }) => [idx, stat]));

			for (const indexName of timeseriesIndexes) {
				expect(statisticsByIndex.get(indexName), indexName).toEqual(expect.any(String));
			}
		} finally {
			database.close();
		}
	});
});
