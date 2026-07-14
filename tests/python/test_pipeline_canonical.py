import importlib
import json
import sqlite3
from datetime import datetime

import pytest


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
        pipeline.FlowFact(
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
        ('literal', 'literal', 1, 3, 300),
    ]


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
