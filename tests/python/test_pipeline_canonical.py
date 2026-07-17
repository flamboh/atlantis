import importlib
import json
import sqlite3
from datetime import datetime

import pytest
from flow_observation import FlowObservation


def load_module():
    pipeline = importlib.import_module('pipeline')
    return importlib.reload(pipeline)


def table_exists(conn: sqlite3.Connection, table_name: str) -> bool:
    return (
        conn.execute(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?",
            (table_name,),
        ).fetchone()
        is not None
    )


def make_raw_bucket(pipeline, source_id: str, bucket_start: int):
    bucket = pipeline.StatisticalBucket(
        pipeline.BucketKey(
            source_id,
            '5m',
            bucket_start,
            bucket_start + pipeline.FIVE_MINUTE_SECONDS,
        )
    )
    bucket.add(
        FlowObservation(
            ip_version=4,
            src_ip='192.0.2.1',
            dst_ip='198.51.100.1',
            protocol=6,
            packets=10,
            bytes_count=1000,
            src_tos=0,
        )
    )
    return bucket.finish()


def test_dataset_tree_config_uses_dataset_db_path(monkeypatch: pytest.MonkeyPatch) -> None:
    pipeline = load_module()
    common = importlib.import_module('common')
    dataset = {
        'dataset_id': 'alpha',
        'root_path': '/captures/alpha',
        'db_path': '/data/custom/alpha.sqlite',
        'sources': [{'source_id': 'r1', 'members': ['r1']}],
    }
    monkeypatch.setattr(common, 'get_dataset_config', lambda dataset_id: dataset)

    config = pipeline.build_dataset_tree_config(dataset_id='alpha', start_date='2025-02-11')
    override = pipeline.build_dataset_tree_config(
        dataset_id='alpha',
        start_date='2025-02-11',
        database_path='/tmp/override.sqlite',
    )

    assert config['database_path'] == '/data/custom/alpha.sqlite'
    assert override['database_path'] == '/tmp/override.sqlite'


def test_apply_cli_config_overrides_updates_loaded_config() -> None:
    pipeline = load_module()
    config = {
        'database_path': '/old.sqlite',
        'maad_bin': '/old-maad',
        'max_workers': 1,
        'inputs': [{'input_kind': 'nfcapd_tree'}],
    }

    pipeline.apply_cli_config_overrides(
        config,
        database_path='/new.sqlite',
        maad_bin='/new-maad',
        max_workers=4,
        force=True,
    )

    assert config['database_path'] == '/new.sqlite'
    assert config['maad_bin'] == '/new-maad'
    assert config['max_workers'] == 4
    assert config['inputs'][0]['force'] is True


def test_force_config_override_requires_nfcapd_tree_input() -> None:
    pipeline = load_module()

    with pytest.raises(ValueError, match='nfcapd_tree'):
        pipeline.apply_cli_config_overrides({'inputs': [{'input_kind': 'csv'}]}, force=True)


def test_force_config_override_rejects_multiple_nfcapd_tree_inputs() -> None:
    pipeline = load_module()

    with pytest.raises(ValueError, match='exactly one'):
        pipeline.apply_cli_config_overrides(
            {
                'inputs': [
                    {'input_kind': 'nfcapd_tree'},
                    {'input_kind': 'nfcapd_tree'},
                ]
            },
            force=True,
        )


def test_worker_count_overrides_must_be_positive(monkeypatch: pytest.MonkeyPatch) -> None:
    pipeline = load_module()
    common = importlib.import_module('common')
    dataset = {
        'dataset_id': 'alpha',
        'root_path': '/captures/alpha',
        'db_path': '/data/custom/alpha.sqlite',
        'sources': [{'source_id': 'r1', 'members': ['r1']}],
    }
    monkeypatch.setattr(common, 'get_dataset_config', lambda dataset_id: dataset)

    with pytest.raises(ValueError, match='max_workers'):
        pipeline.apply_cli_config_overrides({'inputs': []}, max_workers=0)
    with pytest.raises(ValueError, match='max_workers'):
        pipeline.build_dataset_tree_config(
            dataset_id='alpha',
            start_date='2025-02-11',
            max_workers=0,
        )


def test_time_window_must_align_to_aggregate_boundaries() -> None:
    pipeline = load_module()
    aligned = pipeline.parse_optional_config_time('2025-02-11T00:00')
    partial = pipeline.parse_optional_config_time('2025-02-11T00:05')
    hour = pipeline.parse_optional_config_time('2025-02-11T01:00')
    start_date = pipeline.parse_config_date('2025-02-11')
    end_date = pipeline.parse_config_date('2025-02-11')

    pipeline.validate_aggregate_safe_time_window(
        aligned,
        pipeline.local_midnight_epoch(end_date + pipeline.timedelta(days=1)),
        start_date=start_date,
        end_date=end_date,
    )
    with pytest.raises(ValueError, match='30m boundary'):
        pipeline.validate_aggregate_safe_time_window(
            partial,
            None,
            start_date=start_date,
            end_date=end_date,
        )
    with pytest.raises(ValueError, match='1d boundary'):
        pipeline.validate_aggregate_safe_time_window(
            hour,
            None,
            start_date=start_date,
            end_date=end_date,
        )


def test_time_window_must_be_non_empty_and_within_selected_dates() -> None:
    pipeline = load_module()
    day_start = pipeline.parse_optional_config_time('2025-02-11T00:00')
    next_day_start = pipeline.parse_optional_config_time('2025-02-12T00:00')
    previous_day_start = pipeline.parse_optional_config_time('2025-02-10T00:00')
    start_date = pipeline.parse_config_date('2025-02-11')
    end_date = pipeline.parse_config_date('2025-02-11')

    with pytest.raises(ValueError, match='non-empty'):
        pipeline.validate_aggregate_safe_time_window(
            day_start,
            day_start,
            start_date=start_date,
            end_date=end_date,
        )
    with pytest.raises(ValueError, match='on or after'):
        pipeline.validate_aggregate_safe_time_window(
            previous_day_start,
            next_day_start,
            start_date=start_date,
            end_date=end_date,
        )
    with pytest.raises(ValueError, match='on or before'):
        pipeline.validate_aggregate_safe_time_window(
            day_start,
            pipeline.parse_optional_config_time('2025-02-13T00:00'),
            start_date=start_date,
            end_date=end_date,
        )


def test_headerless_timestamp_ordered_csv_accumulates_src_tos_with_arrow(tmp_path) -> None:
    pipeline = load_module()
    conn = sqlite3.connect(':memory:')
    csv_path = tmp_path / 'flows.csv'
    mapping_path = tmp_path / 'mapping.json'
    csv_path.write_text(
        '\n'.join(
            [
                '2016-07-27 13:43:30,42.219.154.107,143.72.8.137,6,3,300,3',
                '2016-07-27 13:44:00,42.219.154.108,143.72.8.138,17,2,200,0',
            ]
        )
        + '\n',
        encoding='utf-8',
    )
    mapping_path.write_text(
        json.dumps(
            {
                'has_header': False,
                'timestamp_format': 'datetime',
                'timestamp_timezone': 'UTC',
                'fieldnames': ['te', 'src', 'dst', 'proto', 'packets', 'bytes', 'stos'],
                'columns': {
                    'time_end': 'te',
                    'src_ip': 'src',
                    'dst_ip': 'dst',
                    'protocol': 'proto',
                    'packets': 'packets',
                    'bytes': 'bytes',
                    'src_tos': 'stos',
                },
                'source_id': {'value': 'ugr16'},
            }
        ),
        encoding='utf-8',
    )

    pipeline.process_input_specs(
        conn,
        [
            {
                'input_kind': 'csv',
                'path': str(csv_path),
                'mapping_path': str(mapping_path),
            }
        ],
        maad_bin='',
        maad_backend='python',
        max_workers=1,
        run_maad=False,
    )

    traffic_rows = conn.execute(
        """
        SELECT src_visibility, dst_visibility, flows, packets, bytes
        FROM traffic_stats
        WHERE source_id = 'ugr16'
          AND granularity = '5m'
          AND ip_version = 4
        ORDER BY src_visibility, dst_visibility
        """
    ).fetchall()

    assert traffic_rows == [
        ('all', 'all', 2, 5, 500),
        ('anonymized', 'anonymized', 1, 3, 300),
        ('anonymized', 'literal', 0, 0, 0),
        ('literal', 'anonymized', 0, 0, 0),
        ('literal', 'literal', 1, 2, 200),
    ]


def test_headerless_timestamp_ordered_csv_without_src_tos_uses_default_visibility(tmp_path) -> None:
    pipeline = load_module()
    conn = sqlite3.connect(':memory:')
    csv_path = tmp_path / 'flows.csv'
    mapping_path = tmp_path / 'mapping.json'
    csv_path.write_text(
        '2016-07-27 13:43:30,42.219.154.107,143.72.8.137,6,3,300\n',
        encoding='utf-8',
    )
    mapping_path.write_text(
        json.dumps(
            {
                'has_header': False,
                'timestamp_format': 'datetime',
                'timestamp_timezone': 'UTC',
                'fieldnames': ['te', 'src', 'dst', 'proto', 'packets', 'bytes'],
                'columns': {
                    'time_end': 'te',
                    'src_ip': 'src',
                    'dst_ip': 'dst',
                    'protocol': 'proto',
                    'packets': 'packets',
                    'bytes': 'bytes',
                },
                'source_id': {'value': 'ugr16'},
            }
        ),
        encoding='utf-8',
    )

    pipeline.process_input_specs(
        conn,
        [
            {
                'input_kind': 'csv',
                'path': str(csv_path),
                'mapping_path': str(mapping_path),
            }
        ],
        maad_bin='',
        maad_backend='python',
        max_workers=1,
        run_maad=False,
    )

    traffic_rows = conn.execute(
        """
        SELECT src_visibility, dst_visibility, flows, packets, bytes
        FROM traffic_stats
        WHERE source_id = 'ugr16'
          AND granularity = '5m'
          AND ip_version = 4
        ORDER BY src_visibility, dst_visibility
        """
    ).fetchall()

    assert traffic_rows == [
        ('all', 'all', 1, 3, 300),
        ('anonymized', 'anonymized', 0, 0, 0),
        ('anonymized', 'literal', 0, 0, 0),
        ('literal', 'anonymized', 0, 0, 0),
        ('literal', 'literal', 1, 3, 300),
    ]


def test_unsorted_csv_uses_deep_scan_with_default_workers_and_persists_zero_gap(tmp_path) -> None:
    pipeline = load_module()
    conn = sqlite3.connect(':memory:')
    csv_path = tmp_path / 'flows.csv'
    mapping_path = tmp_path / 'mapping.json'
    csv_path.write_text(
        '2016-07-27 13:50:00,192.0.2.2,198.51.100.2,17,1,20,0\n'
        '2016-07-27 13:40:00,192.0.2.1,198.51.100.1,6,1,10,0\n',
        encoding='utf-8',
    )
    mapping_path.write_text(
        json.dumps(
            {
                'has_header': False,
                'input_order': 'unsorted',
                'timestamp_format': 'datetime',
                'timestamp_timezone': 'UTC',
                'fieldnames': ['te', 'src', 'dst', 'proto', 'packets', 'bytes', 'stos'],
                'columns': {
                    'time_end': 'te',
                    'src_ip': 'src',
                    'dst_ip': 'dst',
                    'protocol': 'proto',
                    'packets': 'packets',
                    'bytes': 'bytes',
                    'src_tos': 'stos',
                },
                'source_id': {'value': 'r1'},
            }
        ),
        encoding='utf-8',
    )

    pipeline.process_input_specs(
        conn,
        [{'input_kind': 'csv', 'path': str(csv_path), 'mapping_path': str(mapping_path)}],
        maad_bin='',
        maad_backend='python',
        run_maad=False,
    )

    assert conn.execute(
        "SELECT bucket_start, flows FROM traffic_stats WHERE source_id = 'r1' "
        "AND granularity = '5m' AND ip_version = 4 "
        "AND src_visibility = 'all' AND dst_visibility = 'all' ORDER BY bucket_start"
    ).fetchall() == [
        (1469626800, 1),
        (1469627100, 0),
        (1469627400, 1),
    ]
    assert conn.execute(
        "SELECT status, rejected_rows FROM processed_input_scans WHERE input_locator = ?",
        (str(csv_path),),
    ).fetchone() == ('processed', 0)


def test_gap_input_writes_only_canonical_stats_tables() -> None:
    pipeline = load_module()
    conn = sqlite3.connect(':memory:')

    pipeline.process_input_specs(
        conn,
        [
            {
                'input_kind': 'nfcapd',
                'path': '/captures/missing/nfcapd.202504150005',
                'source_id': 'oh_ir1_gw',
                'bucket_start': 1744700700,
                'gap': True,
            }
        ],
        run_maad=False,
        max_workers=1,
    )

    assert table_exists(conn, 'processed_inputs')
    assert table_exists(conn, 'traffic_stats')
    assert table_exists(conn, 'protocol_stats')
    assert table_exists(conn, 'address_count_stats')
    assert table_exists(conn, 'address_structure_stats')
    assert not table_exists(conn, 'netflow_stats_v2')
    assert not table_exists(conn, 'ip_stats_v2')
    assert not table_exists(conn, 'protocol_stats_v2')
    assert not table_exists(conn, 'structure_stats_v2')

    traffic_rows = conn.execute(
        """
        SELECT source_id, granularity, bucket_start, ip_version, src_visibility, dst_visibility, flows
        FROM traffic_stats
        WHERE granularity = '5m'
        ORDER BY ip_version, src_visibility, dst_visibility
        """
    ).fetchall()
    assert traffic_rows == [
        ('oh_ir1_gw', '5m', 1744700700, 4, 'all', 'all', 0),
        ('oh_ir1_gw', '5m', 1744700700, 4, 'anonymized', 'anonymized', 0),
        ('oh_ir1_gw', '5m', 1744700700, 4, 'anonymized', 'literal', 0),
        ('oh_ir1_gw', '5m', 1744700700, 4, 'literal', 'anonymized', 0),
        ('oh_ir1_gw', '5m', 1744700700, 4, 'literal', 'literal', 0),
        ('oh_ir1_gw', '5m', 1744700700, 6, 'all', 'all', 0),
        ('oh_ir1_gw', '5m', 1744700700, 6, 'anonymized', 'anonymized', 0),
        ('oh_ir1_gw', '5m', 1744700700, 6, 'anonymized', 'literal', 0),
        ('oh_ir1_gw', '5m', 1744700700, 6, 'literal', 'anonymized', 0),
        ('oh_ir1_gw', '5m', 1744700700, 6, 'literal', 'literal', 0),
    ]

    address_count_rows = conn.execute(
        """
        SELECT ip_version, src_visibility, dst_visibility, address_side, unique_address_count
        FROM address_count_stats
        WHERE granularity = '5m'
        ORDER BY ip_version, address_side
        """
    ).fetchall()
    assert len(address_count_rows) == 20
    assert set(address_count_rows) == {
        (ip_version, src_visibility, dst_visibility, address_side, 0)
        for ip_version in (4, 6)
        for src_visibility, dst_visibility in pipeline.ZERO_FILL_VISIBILITY_PAIRS
        for address_side in ('source', 'destination')
    }

    processed = conn.execute(
        'SELECT input_kind, source_id, bucket_start, status FROM processed_inputs'
    ).fetchone()
    assert processed == ('nfcapd', 'oh_ir1_gw', 1744700700, 'processed')


def test_fatal_long_csv_then_corrected_shorter_retry_removes_stale_buckets(tmp_path) -> None:
    pipeline = load_module()
    conn = sqlite3.connect(':memory:')
    mapping_path = tmp_path / 'mapping.json'
    mapping_path.write_text(
        json.dumps(
            {
                'has_header': True,
                'timestamp_format': 'datetime',
                'timestamp_timezone': 'UTC',
                'columns': {
                    'time_end': 'te',
                    'src_ip': 'src',
                    'dst_ip': 'dst',
                    'protocol': 'proto',
                    'packets': 'packets',
                    'bytes': 'bytes',
                    'src_tos': 'stos',
                },
                'source_id': {'value': 'r1'},
                'input_order': 'timestamp_ascending',
                'out_of_order_lag_buckets': 0,
            }
        ),
        encoding='utf-8',
    )
    csv_path = tmp_path / 'flows.csv'
    csv_path.write_text(
        'te,src,dst,proto,packets,bytes,stos\n'
        '2016-07-27 13:40:00,192.0.2.1,198.51.100.1,6,1,2,0\n'
        '2016-07-27 14:50:00,192.0.2.2,198.51.100.2,17,1,2,0\n'
        '2016-07-27 16:00:00,192.0.2.3,198.51.100.3,6,1,2,0\n'
        '2016-07-27 16:05:00,too,few\n',
        encoding='utf-8',
    )
    spec = {
        'input_kind': 'csv',
        'path': str(csv_path),
        'mapping_path': str(mapping_path),
    }

    with pytest.raises(ValueError, match='column count'):
        pipeline.process_input_specs(conn, [spec], run_maad=False)

    assert conn.execute(
        """
        SELECT COUNT(*) FROM processed_inputs
        WHERE input_kind = 'csv' AND scan_locator = ?
        """,
        (str(csv_path),),
    ).fetchone() == (0,)
    assert conn.execute('SELECT * FROM processed_input_scans').fetchall() == []
    for table_name in (
        'traffic_stats',
        'protocol_stats',
        'address_count_stats',
        'address_structure_stats',
    ):
        assert conn.execute(f'SELECT COUNT(*) FROM {table_name}').fetchone() == (0,)

    csv_path.write_text(
        'te,src,dst,proto,packets,bytes,stos\n'
        '2016-07-27 13:40:00,192.0.2.1,198.51.100.1,6,1,2,0\n',
        encoding='utf-8',
    )
    pipeline.process_input_specs(conn, [spec], run_maad=False)

    expected_bucket = 1469626800
    assert conn.execute(
        """
        SELECT bucket_start, status FROM processed_inputs
        WHERE input_kind = 'csv' AND scan_locator = ?
        """,
        (str(csv_path),),
    ).fetchall() == [(expected_bucket, 'processed')]
    assert conn.execute(
        """
        SELECT DISTINCT bucket_start FROM traffic_stats
        WHERE source_id = 'r1' AND granularity = '5m'
        """
    ).fetchall() == [(expected_bucket,)]
    assert conn.execute(
        """
        SELECT status FROM processed_input_scans
        WHERE input_kind = 'csv' AND input_locator = ?
        """,
        (str(csv_path),),
    ).fetchone() == ('processed',)


def test_csv_batch_failure_cleans_every_started_nonterminal_scan(tmp_path) -> None:
    pipeline = load_module()
    conn = sqlite3.connect(':memory:')
    mapping_path = tmp_path / 'mapping.json'
    mapping_path.write_text(
        json.dumps(
            {
                'has_header': True,
                'timestamp_format': 'datetime',
                'timestamp_timezone': 'UTC',
                'columns': {
                    'time_end': 'te',
                    'src_ip': 'src',
                    'dst_ip': 'dst',
                    'protocol': 'proto',
                    'packets': 'packets',
                    'bytes': 'bytes',
                    'src_tos': 'stos',
                },
                'source_id': {'value': 'r1'},
                'input_order': 'timestamp_ascending',
            }
        ),
        encoding='utf-8',
    )
    good_path = tmp_path / 'a-good.csv'
    bad_path = tmp_path / 'b-bad.csv'
    good_path.write_text(
        'te,src,dst,proto,packets,bytes,stos\n'
        '2016-07-27 13:40:00,192.0.2.1,198.51.100.1,6,1,2,0\n',
        encoding='utf-8',
    )
    bad_path.write_text(
        'te,src\n'
        '2016-07-27 13:45:00,192.0.2.2\n',
        encoding='utf-8',
    )
    specs = [
        {
            'input_kind': 'csv',
            'path': str(good_path),
            'mapping_path': str(mapping_path),
        },
        {
            'input_kind': 'csv',
            'path': str(bad_path),
            'mapping_path': str(mapping_path),
        },
    ]

    with pytest.raises(ValueError, match='missing mapped columns'):
        pipeline.process_input_specs(conn, specs, run_maad=False)

    assert conn.execute('SELECT COUNT(*) FROM processed_inputs').fetchone() == (0,)
    assert conn.execute('SELECT COUNT(*) FROM processed_input_scans').fetchone() == (0,)
    for table_name in (
        'traffic_stats',
        'protocol_stats',
        'address_count_stats',
        'address_structure_stats',
    ):
        assert conn.execute(f'SELECT COUNT(*) FROM {table_name}').fetchone() == (0,)

    bad_path.write_text(
        'te,src,dst,proto,packets,bytes,stos\n'
        '2016-07-27 13:45:00,192.0.2.2,198.51.100.2,17,3,4,0\n',
        encoding='utf-8',
    )
    pipeline.process_input_specs(conn, specs, run_maad=False)

    assert conn.execute(
        "SELECT COUNT(*) FROM processed_inputs WHERE status = 'processed'"
    ).fetchone() == (2,)
    assert conn.execute(
        "SELECT COUNT(*) FROM processed_input_scans WHERE status = 'processed'"
    ).fetchone() == (2,)
    assert conn.execute(
        """
        SELECT DISTINCT bucket_start FROM traffic_stats
        WHERE source_id = 'r1' AND granularity = '5m'
        ORDER BY bucket_start
        """
    ).fetchall() == [(1469626800,), (1469627100,)]


def test_tree_zero_fill_covers_explicit_multi_day_blank_interval(tmp_path) -> None:
    pipeline = load_module()
    conn = sqlite3.connect(':memory:')
    root = tmp_path / 'captures'
    anchor_dir = root / 'r1' / '2025' / '11' / '16'
    anchor_dir.mkdir(parents=True)
    (anchor_dir / 'nfcapd.202511162355').touch()

    pipeline.process_nfcapd_tree_spec(
        conn,
        {
            'root_path': str(root),
            'source_ids': ['r1'],
            'start_date': '2025-11-17',
            'end_date': '2025-11-24',
            'zero_fill_gaps': True,
        },
        maad_bin='',
        maad_backend='python',
        maad_workers=1,
        max_workers=1,
        run_maad=False,
    )

    expected_buckets = {
        bucket_start
        for day in (
            datetime(2025, 11, 17),
            datetime(2025, 11, 18),
            datetime(2025, 11, 19),
            datetime(2025, 11, 20),
            datetime(2025, 11, 21),
            datetime(2025, 11, 22),
            datetime(2025, 11, 23),
            datetime(2025, 11, 24),
        )
        for bucket_start in pipeline.iter_local_day_bucket_starts(day)
    }

    traffic_buckets = {
        row[0]
        for row in conn.execute(
            """
            SELECT bucket_start
            FROM traffic_stats
            WHERE source_id = 'r1'
              AND granularity = '5m'
              AND ip_version = 4
              AND src_visibility = 'all'
              AND dst_visibility = 'all'
              AND flows = 0
              AND packets = 0
              AND bytes = 0
            """
        ).fetchall()
    }
    processed_buckets = {
        row[0]
        for row in conn.execute(
            """
            SELECT bucket_start
            FROM processed_inputs
            WHERE source_id = 'r1'
              AND input_kind = 'nfcapd'
              AND status = 'processed'
            """
        ).fetchall()
    }

    assert traffic_buckets == expected_buckets
    assert processed_buckets == expected_buckets

    daily_exact_buckets = {
        row[0]
        for row in conn.execute(
            """
            SELECT bucket_start
            FROM traffic_stats
            WHERE source_id = 'r1'
              AND granularity = '1d'
              AND ip_version = 4
              AND src_visibility = 'literal'
              AND dst_visibility = 'literal'
              AND flows = 0
              AND packets = 0
              AND bytes = 0
            """
        ).fetchall()
    }
    assert daily_exact_buckets == {
        next(iter(pipeline.iter_local_day_bucket_starts(day)))
        for day in (
            datetime(2025, 11, 17),
            datetime(2025, 11, 18),
            datetime(2025, 11, 19),
            datetime(2025, 11, 20),
            datetime(2025, 11, 21),
            datetime(2025, 11, 22),
            datetime(2025, 11, 23),
            datetime(2025, 11, 24),
        )
    }


def test_partial_logical_rewrite_does_not_replace_existing_aggregate_with_slice() -> None:
    pipeline = load_module()
    conn = sqlite3.connect(':memory:')
    base_bucket = 1744700400
    raw_buckets = [
        make_raw_bucket(pipeline, 'r1', base_bucket + offset * pipeline.FIVE_MINUTE_SECONDS)
        for offset in range(6)
    ]
    pipeline.init_stats_tables(conn)
    pipeline.bind_current_product(conn, run_maad=False, maad_backend='python')
    pipeline.write_aggregate_rows(
        conn,
        raw_buckets,
        maad_bin='',
        max_workers=1,
        maad_backend='python',
        run_maad=False,
    )

    pipeline.process_nfcapd_logical_bucket_jobs(
        conn,
        [
            {
                'source_id': 'r1',
                'bucket_start': base_bucket,
                'bucket_end': base_bucket + pipeline.FIVE_MINUTE_SECONDS,
                'member_specs': [],
                'missing_members': ['r1'],
            }
        ],
        maad_bin='',
        maad_backend='python',
        maad_workers=1,
        max_workers=1,
        run_maad=False,
    )

    aggregate_flows = conn.execute(
        """
        SELECT flows
        FROM traffic_stats
        WHERE source_id = 'r1'
          AND granularity = '30m'
          AND bucket_start = ?
          AND ip_version = 4
          AND src_visibility = 'all'
          AND dst_visibility = 'all'
        """,
        (base_bucket,),
    ).fetchone()
    rewritten_5m = conn.execute(
        """
        SELECT flows
        FROM traffic_stats
        WHERE source_id = 'r1'
          AND granularity = '5m'
          AND bucket_start = ?
          AND ip_version = 4
          AND src_visibility = 'all'
          AND dst_visibility = 'all'
        """,
        (base_bucket,),
    ).fetchone()

    assert aggregate_flows == (6,)
    assert rewritten_5m == (0,)


def test_logical_force_replaces_changed_owner_but_rejects_unrelated_csv() -> None:
    pipeline = load_module()
    conn = sqlite3.connect(':memory:')
    pipeline.init_processed_inputs_table(conn)
    pipeline.init_stats_tables(conn)
    pipeline.bind_current_product(conn, run_maad=False, maad_backend='python')
    locator = '/captures/nfcapd.202504150000'
    old_revision = pipeline.InputRevision.create(
        input_kind='nfcapd',
        locator=locator,
        content_fingerprint='old',
        decoder_fingerprint='decoder',
    )
    pipeline.upsert_input_bucket(
        conn,
        input_kind='nfcapd',
        input_locator=locator,
        source_id='r1',
        bucket_start=0,
        bucket_end=300,
        input_revision=old_revision,
    )
    pipeline.mark_input_bucket_status(
        conn,
        input_kind='nfcapd',
        input_locator=locator,
        source_id='r1',
        bucket_start=0,
        status='processed',
        input_revision=old_revision,
    )
    removed_locator = '/captures/nfcapd.202504150000.removed-member'
    removed_revision = pipeline.InputRevision.create(
        input_kind='nfcapd',
        locator=removed_locator,
        content_fingerprint='removed',
        decoder_fingerprint='decoder',
    )
    pipeline.upsert_input_bucket(
        conn,
        input_kind='nfcapd',
        input_locator=removed_locator,
        source_id='r1',
        bucket_start=0,
        bucket_end=300,
        input_revision=removed_revision,
    )
    pipeline.mark_input_bucket_status(
        conn,
        input_kind='nfcapd',
        input_locator=removed_locator,
        source_id='r1',
        bucket_start=0,
        status='processed',
        input_revision=removed_revision,
    )
    new_revision = pipeline.InputRevision.create(
        input_kind='nfcapd',
        locator=locator,
        content_fingerprint='new',
        decoder_fingerprint='decoder',
    )
    job = {
        'source_id': 'r1',
        'bucket_start': 0,
        'bucket_end': 300,
        'member_specs': [
            {
                'input_kind': 'nfcapd',
                'path': locator,
                'source_id': 'member',
                'input_revision': new_revision,
            }
        ],
        'missing_members': [],
    }
    raw = make_raw_bucket(pipeline, 'member', 0)
    pipeline.process_logical_nfcapd_job(
        conn,
        job,
        {locator: raw},
        maad_bin='',
        maad_backend='python',
        aggregate_buckets={},
        processed_buckets=[],
        current_run_keys=set(),
        delete_existing=True,
    )
    assert conn.execute(
        "SELECT input_locator, content_fingerprint FROM processed_inputs "
        "WHERE input_kind = 'nfcapd'"
    ).fetchall() == [(locator, 'new')]

    csv_revision = pipeline.InputRevision.create(
        input_kind='csv',
        locator='/csv/other.csv',
        content_fingerprint='csv',
        decoder_fingerprint='decoder',
    )
    pipeline.upsert_input_bucket(
        conn,
        input_kind='csv',
        input_locator=csv_revision.locator,
        source_id='r1',
        bucket_start=0,
        bucket_end=300,
        input_revision=csv_revision,
    )
    with pytest.raises(pipeline.AggregatePublicationConflict, match='Overlapping'):
        pipeline.process_logical_nfcapd_job(
            conn,
            job,
            {locator: raw},
            maad_bin='',
            maad_backend='python',
            aggregate_buckets={},
            processed_buckets=[],
            current_run_keys=set(),
            delete_existing=True,
        )


def test_gap_publication_rejects_real_file_appearing_after_discovery(tmp_path) -> None:
    pipeline = load_module()
    conn = sqlite3.connect(':memory:')
    expected_path = tmp_path / 'nfcapd.202504150000'
    absence = pipeline.ExpectedAbsence.capture(expected_path)
    job = {
        'source_id': 'r1',
        'bucket_start': 0,
        'bucket_end': 300,
        'member_specs': [],
        'missing_members': ['r1'],
        'absence_snapshots': [absence],
    }
    expected_path.write_bytes(b'appeared after discovery')

    with pytest.raises(RuntimeError, match='appeared before gap publication'):
        pipeline.process_nfcapd_logical_bucket_jobs(
            conn,
            [job],
            maad_bin='',
            maad_backend='python',
            maad_workers=1,
            max_workers=1,
            run_maad=False,
        )

    assert conn.execute('SELECT COUNT(*) FROM processed_inputs').fetchone() == (0,)
    assert conn.execute('SELECT COUNT(*) FROM traffic_stats').fetchone() == (0,)


def test_gap_final_absence_check_rolls_back_replacement(tmp_path, monkeypatch) -> None:
    pipeline = load_module()
    conn = sqlite3.connect(':memory:')
    pipeline.init_processed_inputs_table(conn)
    pipeline.init_stats_tables(conn)
    old_payload = pipeline.build_nfcapd_gap_payload(
        'gap://nfcapd/old',
        'r1',
        0,
        run_maad=False,
    )
    pipeline.write_input_payload(conn, old_payload)

    absence = pipeline.ExpectedAbsence.capture(tmp_path / 'expected')
    replacement = pipeline.build_nfcapd_gap_payload(
        'gap://nfcapd/new',
        'r1',
        0,
        run_maad=False,
    )
    replacement['absence_snapshots'] = [absence]
    calls = 0

    def fail_final_check(_self):
        nonlocal calls
        calls += 1
        if calls == 2:
            raise RuntimeError('appeared during publication')

    monkeypatch.setattr(pipeline.ExpectedAbsence, 'verify', fail_final_check)
    with pytest.raises(RuntimeError, match='during publication'):
        pipeline.write_input_payload(conn, replacement, delete_existing=True)

    assert conn.execute(
        "SELECT input_locator, status FROM processed_inputs WHERE source_id = 'r1'"
    ).fetchall() == [('gap://nfcapd/old', 'processed')]
    assert conn.execute(
        "SELECT COUNT(*) FROM traffic_stats WHERE source_id = 'r1' AND granularity = '5m'"
    ).fetchone()[0] > 0


def test_batch_and_streaming_aggregation_render_identical_rows() -> None:
    pipeline = load_module()
    children = [
        make_raw_bucket(pipeline, 'r1', bucket_start)
        for bucket_start in (0, pipeline.FIVE_MINUTE_SECONDS)
    ]

    batch_rows = pipeline.build_aggregate_stats_payloads(children)
    batch_by_key = {
        (
            payload['traffic_rows'][0]['granularity'],
            payload['traffic_rows'][0]['bucket_start'],
        ): payload
        for payload in batch_rows
    }
    streaming = {}
    for child in children:
        pipeline.add_raw_bucket_to_streaming_aggregates(streaming, child)
    streaming_by_key = {
        (granularity, bucket_start): pipeline.canonical_bucket_rows(builder.finish())
        for (_source_id, granularity, bucket_start), builder in streaming.items()
    }

    assert streaming_by_key == batch_by_key
