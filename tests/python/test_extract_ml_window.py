import importlib
import sqlite3
from datetime import datetime
from pathlib import Path

import pytest


def load_module():
    module = importlib.import_module('extract_ml_window')
    return importlib.reload(module)


def make_source_db(path: Path) -> sqlite3.Connection:
    conn = sqlite3.connect(path)
    conn.row_factory = sqlite3.Row
    conn.execute(
        'CREATE TABLE netflow_stats (timestamp INTEGER NOT NULL, router TEXT NOT NULL, flows INTEGER NOT NULL)'
    )
    conn.execute('CREATE INDEX idx_netflow_timestamp ON netflow_stats(timestamp)')
    conn.executemany(
        'INSERT INTO netflow_stats (timestamp, router, flows) VALUES (?, ?, ?)',
        [
            (100, 'r1', 10),
            (200, 'r1', 20),
            (400, 'r2', 40),
        ],
    )
    conn.commit()
    return conn


def make_v2_source_db(path: Path) -> sqlite3.Connection:
    conn = sqlite3.connect(path)
    conn.row_factory = sqlite3.Row
    conn.execute(
        'CREATE TABLE datasets (id TEXT PRIMARY KEY NOT NULL, label TEXT NOT NULL, default_start_date TEXT NOT NULL)'
    )
    conn.execute(
        """
        CREATE TABLE netflow_stats_v2 (
            source_id TEXT NOT NULL,
            granularity TEXT NOT NULL,
            bucket_start INTEGER NOT NULL,
            bucket_end INTEGER NOT NULL,
            ip_version INTEGER NOT NULL,
            flows INTEGER NOT NULL,
            PRIMARY KEY (source_id, granularity, bucket_start, ip_version)
        ) WITHOUT ROWID
        """
    )
    conn.execute(
        'CREATE INDEX idx_netflow_stats_v2_granularity_bucket_source ON netflow_stats_v2(granularity, bucket_start, source_id, ip_version)'
    )
    conn.execute(
        'INSERT INTO datasets (id, label, default_start_date) VALUES (?, ?, ?)',
        ('uoregon', 'UONet-in v2', '2025-02-01'),
    )
    conn.executemany(
        'INSERT INTO netflow_stats_v2 (source_id, granularity, bucket_start, bucket_end, ip_version, flows) VALUES (?, ?, ?, ?, ?, ?)',
        [
            ('r1', '5m', 100, 400, 4, 10),
            ('r1', '5m', 200, 500, 6, 20),
            ('r2', '5m', 200, 500, 4, 30),
            ('r1', '5m', 500, 800, 4, 50),
        ],
    )
    conn.commit()
    return conn


def test_compute_window_rejects_reversed_dates() -> None:
    module = load_module()
    with pytest.raises(SystemExit, match='--end-exclusive must be after --start'):
        module.compute_window('2025-03-02', '2025-03-02', 'America/Los_Angeles')


def test_compute_window_uses_exclusive_local_date_boundary() -> None:
    module = load_module()
    start_dt, end_dt, start_ts, end_ts = module.compute_window(
        '2025-05-01',
        '2026-05-01',
        'America/Los_Angeles',
    )

    assert start_dt.strftime('%Y-%m-%d') == '2025-05-01'
    assert end_dt.strftime('%Y-%m-%d') == '2026-05-01'
    assert start_ts == 1746082800
    assert end_ts == 1777618800


def test_copy_table_to_sqlite_preserves_schema_and_rows(tmp_path: Path) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    dest_path = tmp_path / 'dest.sqlite'
    source_conn = make_source_db(source_path)
    dest_conn = module.connect_db(dest_path)

    module.create_sqlite_table(source_conn, dest_conn, 'netflow_stats')
    inserted = module.copy_table_to_sqlite(
        source_conn,
        dest_conn,
        'netflow_stats',
        'timestamp',
        150,
        500,
        2,
    )

    assert inserted == 2
    rows = dest_conn.execute(
        'SELECT timestamp, router, flows FROM netflow_stats ORDER BY timestamp'
    ).fetchall()
    assert [tuple(row) for row in rows] == [(200, 'r1', 20), (400, 'r2', 40)]


def test_copy_v2_table_filters_by_source_id(tmp_path: Path) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    dest_path = tmp_path / 'dest.sqlite'
    source_conn = make_v2_source_db(source_path)
    dest_conn = module.connect_db(dest_path)
    config = module.TABLE_CONFIG['netflow_stats_v2']
    extra_where, extra_params = module.build_table_filters(
        source_id='r1',
        granularities=None,
        config=config,
    )

    module.create_sqlite_table(source_conn, dest_conn, 'netflow_stats_v2')
    inserted = module.copy_table_to_sqlite(
        source_conn,
        dest_conn,
        'netflow_stats_v2',
        config['time_column'],
        150,
        500,
        2,
        extra_where=extra_where,
        extra_params=extra_params,
    )

    assert inserted == 1
    rows = dest_conn.execute(
        'SELECT source_id, granularity, bucket_start, ip_version, flows FROM netflow_stats_v2'
    ).fetchall()
    assert [tuple(row) for row in rows] == [('r1', '5m', 200, 6, 20)]


def test_copy_static_metadata_table_without_time_filter(tmp_path: Path) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    dest_path = tmp_path / 'dest.sqlite'
    source_conn = make_v2_source_db(source_path)
    dest_conn = module.connect_db(dest_path)

    module.create_sqlite_table(source_conn, dest_conn, 'datasets')
    inserted = module.copy_table_to_sqlite(
        source_conn,
        dest_conn,
        'datasets',
        None,
        150,
        500,
        2,
    )

    assert inserted == 1
    row = dest_conn.execute('SELECT id, label, default_start_date FROM datasets').fetchone()
    assert tuple(row) == ('uoregon', 'UONet-in v2', '2025-02-01')


def test_collect_table_summary_and_manifest(tmp_path: Path) -> None:
    module = load_module()
    source_conn = make_source_db(tmp_path / 'summary.sqlite')

    summary = module.collect_table_summary(source_conn, 'netflow_stats', 'timestamp', 50, 250)
    manifest = module.build_manifest(
        source_db=tmp_path / 'summary.sqlite',
        output_dir=tmp_path / 'out',
        start_dt=datetime(2025, 3, 30),
        end_exclusive_dt=datetime(2025, 6, 8),
        start_ts=123,
        end_ts=456,
        timezone='America/Los_Angeles',
        source_id=None,
        granularities=None,
        tables={'netflow_stats': summary},
    )
    manifest_path = module.write_manifest(tmp_path, manifest)

    assert summary == {'row_count': 2, 'min_time': 100, 'max_time': 200}
    assert manifest['end_exclusive_date'] == '2025-06-08'
    assert manifest['timezone'] == 'America/Los_Angeles'
    assert manifest_path.read_text().endswith('\n')
    assert '"row_count": 2' in manifest_path.read_text()
