import json
from pathlib import Path

import pytest

import stats_v3
from extract_window_helpers import (
    PORTABLE_TABLES,
    copied_rows,
    extract_args,
    index_names,
    load_module,
    make_source_db,
    sqlite_table_names,
    table_sql,
    traffic_row,
)


def test_compute_window_rejects_reversed_dates() -> None:
    module = load_module()

    with pytest.raises(SystemExit, match='--end must be after --start'):
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


def test_parse_args_rejects_removed_router_and_end_inclusive(tmp_path: Path) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'

    with pytest.raises(SystemExit):
        module.parse_args(['--source-db', str(source_path), '--router', 'r1'])

    with pytest.raises(SystemExit):
        module.parse_args(['--source-db', str(source_path), '--end-inclusive', '2025-06-30'])

    with pytest.raises(SystemExit):
        module.parse_args(['--source-db', str(source_path), '--end-exclusive', '2025-07-01'])


def test_parse_args_rejects_unknown_output_choice(tmp_path: Path) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'

    with pytest.raises(SystemExit):
        module.parse_args(['--source-db', str(source_path), '--output', 'csv'])


def test_parse_args_uses_end_as_exclusive_boundary(tmp_path: Path) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'

    args = module.parse_args(
        ['--source-db', str(source_path), '--start', '2025-05-01', '--end', '2025-05-02']
    )
    _, _, start_ts, end_ts = module.compute_window(args.start, args.end_exclusive, args.timezone)

    assert start_ts == 1746082800
    assert end_ts == 1746169200


def test_default_output_dir_is_generated_from_dataset_and_window(tmp_path: Path) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'

    args = module.parse_args(
        [
            '--source-db',
            str(source_path),
            '--dataset',
            'uoregon',
            '--start',
            '2025-06-01',
            '--end',
            '2026-06-01',
        ]
    )
    output_dir = Path(args.output_dir).as_posix()

    assert 'uoregon' in output_dir
    assert '2025-06-01' in output_dir
    assert '2026-06-01' in output_dir
    assert not output_dir.endswith('data/uoregon-v2/ml-2025-06-01-to-2026-06-01')


def test_default_output_dir_slug_cannot_escape_data_dir(tmp_path: Path) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'

    args = module.parse_args(
        [
            '--source-db',
            str(source_path),
            '--dataset',
            '..',
            '--start',
            '2025-06-01',
            '--end',
            '2026-06-01',
        ]
    )

    output_dir = Path(args.output_dir)
    assert '..' not in output_dir.parts
    assert output_dir.parts[:2] == ('data', 'all')


def test_copy_canonical_table_preserves_schema_indexes_and_window_boundaries(
    tmp_path: Path,
) -> None:
    module = load_module()
    source_conn = make_source_db(tmp_path / 'source.sqlite')
    dest_conn = module.connect_db(tmp_path / 'dest.sqlite')
    config = module.TABLE_CONFIG['traffic_stats']
    where_sql, params = module.build_filters(
        config=config,
        start_ts=150,
        end_ts=500,
        source_id=None,
        granularities=None,
    )

    module.create_sqlite_table(source_conn, dest_conn, 'traffic_stats')
    inserted = module.copy_table_to_sqlite(
        source_conn,
        dest_conn,
        'traffic_stats',
        where_sql,
        params,
        2,
    )

    assert inserted == 3
    assert table_sql(dest_conn, 'traffic_stats') == table_sql(source_conn, 'traffic_stats')
    assert 'idx_traffic_stats_query' in index_names(dest_conn, 'traffic_stats')
    assert copied_rows(dest_conn) == [
        ('r1', '1h', 200, 40),
        ('r1', '5m', 200, 20),
        ('r2', '5m', 200, 30),
    ]


def test_copy_canonical_table_filters_by_source_id_and_granularity(tmp_path: Path) -> None:
    module = load_module()
    source_conn = make_source_db(tmp_path / 'source.sqlite')
    dest_conn = module.connect_db(tmp_path / 'dest.sqlite')
    config = module.TABLE_CONFIG['traffic_stats']
    where_sql, params = module.build_filters(
        config=config,
        start_ts=150,
        end_ts=500,
        source_id='r1',
        granularities=['5m'],
    )

    module.create_sqlite_table(source_conn, dest_conn, 'traffic_stats')
    inserted = module.copy_table_to_sqlite(
        source_conn,
        dest_conn,
        'traffic_stats',
        where_sql,
        params,
        2,
    )

    assert inserted == 1
    assert copied_rows(dest_conn) == [('r1', '5m', 200, 20)]


def test_table_config_exports_only_portable_analysis_tables() -> None:
    module = load_module()

    assert set(module.TABLE_CONFIG) == PORTABLE_TABLES


def test_default_extract_writes_sqlite_only_and_portable_tables(tmp_path: Path) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    make_source_db(source_path).close()
    output_dir = tmp_path / 'out'

    manifest_path = module.extract(extract_args(module, source_path, output_dir))
    manifest = json.loads(manifest_path.read_text())
    sqlite_path = output_dir / module.SQLITE_FILENAME

    assert manifest_path == output_dir / 'manifest.json'
    assert sqlite_path.exists()
    assert not (output_dir / 'parquet').exists()
    assert manifest['outputs']['sqlite_path'] == str(sqlite_path)
    assert manifest['outputs']['parquet_dir'] is None
    assert Path(manifest['outputs']['sqlite_path']).name == module.SQLITE_FILENAME

    with module.connect_db(sqlite_path) as conn:
        assert sqlite_table_names(conn) == PORTABLE_TABLES
        assert copied_rows(conn) == [
            ('r1', '1h', 200, 40),
            ('r1', '5m', 200, 20),
            ('r2', '5m', 200, 30),
        ]


def test_dry_run_prints_plan_without_snapshot_or_outputs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    make_source_db(source_path).close()
    output_dir = tmp_path / 'out'

    def fail_if_snapshot_is_created(source_db: Path, snapshot_path: Path) -> None:
        raise AssertionError(f'dry-run reached source snapshot for {source_db}')

    monkeypatch.setattr(module, 'create_source_snapshot', fail_if_snapshot_is_created)

    result = module.extract(extract_args(module, source_path, output_dir, '--dry-run'))
    captured = capsys.readouterr()

    assert result is None
    assert not output_dir.exists()
    assert str(source_path) in captured.out
    assert str(output_dir) in captured.out
    assert 'traffic_stats' in captured.out
    assert '150' in captured.out
    assert '500' in captured.out


def test_extract_rejects_source_db_at_manifest_output_path_before_snapshot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module = load_module()
    output_dir = tmp_path / 'out'
    output_dir.mkdir()
    source_path = output_dir / 'manifest.json'
    make_source_db(source_path).close()

    def fail_if_rejection_is_missed(source_db: Path, snapshot_path: Path) -> None:
        raise AssertionError(f'extraction reached source snapshot for {source_db}')

    monkeypatch.setattr(module, 'create_source_snapshot', fail_if_rejection_is_missed)

    with pytest.raises(SystemExit, match='manifest|managed output|--source-db'):
        module.extract(extract_args(module, source_path, output_dir))


def test_failed_extraction_preserves_existing_outputs(tmp_path: Path) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    source_conn = make_source_db(source_path)
    source_conn.execute('DROP TABLE traffic_stats')
    source_conn.commit()
    source_conn.close()

    output_dir = tmp_path / 'out'
    parquet_dir = output_dir / 'parquet'
    output_dir.mkdir()
    parquet_dir.mkdir()
    sqlite_path = output_dir / module.SQLITE_FILENAME
    manifest_path = output_dir / 'manifest.json'
    parquet_sentinel = parquet_dir / 'keep.parquet'

    with module.connect_db(sqlite_path) as existing_conn:
        existing_conn.execute('CREATE TABLE sentinel (value TEXT NOT NULL)')
        existing_conn.execute('INSERT INTO sentinel VALUES (?)', ('keep-db',))
    db_bytes = sqlite_path.read_bytes()
    manifest_path.write_text('{"sentinel": true}\n', encoding='utf-8')
    parquet_sentinel.write_bytes(b'keep-parquet')

    with pytest.raises(SystemExit, match='traffic_stats|canonical|missing'):
        module.extract(
            extract_args(module, source_path, output_dir, '--output', 'sqlite', '--output', 'parquet')
        )

    assert sqlite_path.read_bytes() == db_bytes
    assert manifest_path.read_text(encoding='utf-8') == '{"sentinel": true}\n'
    assert parquet_sentinel.read_bytes() == b'keep-parquet'
    assert sorted(path.name for path in parquet_dir.iterdir()) == ['keep.parquet']


def test_extract_missing_required_table_does_not_create_new_output_dir(tmp_path: Path) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    source_conn = make_source_db(source_path)
    source_conn.execute('DROP TABLE traffic_stats')
    source_conn.commit()
    source_conn.close()
    output_dir = tmp_path / 'fresh-out'

    with pytest.raises(SystemExit, match='traffic_stats|missing'):
        module.extract(extract_args(module, source_path, output_dir))

    assert not output_dir.exists()


def test_dry_run_missing_required_table_does_not_report_success_or_create_output_dir(
    tmp_path: Path,
) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    source_conn = make_source_db(source_path)
    source_conn.execute('DROP TABLE traffic_stats')
    source_conn.commit()
    source_conn.close()
    output_dir = tmp_path / 'fresh-out'

    with pytest.raises(SystemExit, match='traffic_stats|missing'):
        module.extract(extract_args(module, source_path, output_dir, '--dry-run'))

    assert not output_dir.exists()


def test_programmatic_invalid_batch_size_does_not_create_output_dir(tmp_path: Path) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    make_source_db(source_path).close()
    output_dir = tmp_path / 'fresh-out'
    args = extract_args(module, source_path, output_dir)
    args.batch_size = 0

    with pytest.raises(SystemExit, match='--batch-size must be positive'):
        module.extract(args)

    assert not output_dir.exists()


def test_dry_run_missing_required_columns_does_not_report_success_or_create_output_dir(
    tmp_path: Path,
) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    with module.connect_db(source_path) as conn:
        for table in module.TABLE_CONFIG:
            conn.execute(f'CREATE TABLE {table} (not_bucket INTEGER)')
        conn.commit()
    output_dir = tmp_path / 'fresh-out'

    with pytest.raises(SystemExit, match='bucket_start|source_id|granularity'):
        module.extract(extract_args(module, source_path, output_dir, '--dry-run'))

    assert not output_dir.exists()


def test_dry_run_requires_portable_table_payload_columns(tmp_path: Path) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    with module.connect_db(source_path) as conn:
        for table in module.TABLE_CONFIG:
            conn.execute(
                f'CREATE TABLE {table} ('
                'bucket_start INTEGER, source_id TEXT, granularity TEXT)'
            )
        conn.commit()
    output_dir = tmp_path / 'fresh-out'

    with pytest.raises(SystemExit, match='traffic_stats.bytes|protocol_stats.protocols_list'):
        module.extract(extract_args(module, source_path, output_dir, '--dry-run'))

    assert not output_dir.exists()


def test_extract_reads_from_source_snapshot(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    source_conn = make_source_db(source_path)
    source_conn.execute('PRAGMA journal_mode=WAL')
    source_conn.close()

    original_copy_table = module.copy_table_to_sqlite
    mutated = False

    def copy_table_and_mutate_source(
        source_conn,
        dest_conn,
        table: str,
        where_sql: str,
        params: tuple,
        batch_size: int,
    ) -> int:
        nonlocal mutated
        if table == 'traffic_stats' and not mutated:
            mutated = True
            with module.connect_db(source_path) as writer:
                stats_v3.insert_traffic_stats_rows(
                    writer,
                    [
                        traffic_row(source_id='r1', granularity='5m', bucket_start=300, flows=999)
                    ],
                )
                writer.commit()
        inserted = original_copy_table(source_conn, dest_conn, table, where_sql, params, batch_size)
        return inserted

    monkeypatch.setattr(module, 'copy_table_to_sqlite', copy_table_and_mutate_source)

    module.extract(
        extract_args(
            module,
            source_path,
            tmp_path / 'out',
            '--source-id',
            'r1',
            '--granularity',
            '5m',
        )
    )

    with module.connect_db(tmp_path / 'out' / module.SQLITE_FILENAME) as dest_conn:
        assert copied_rows(dest_conn) == [('r1', '5m', 200, 20)]
