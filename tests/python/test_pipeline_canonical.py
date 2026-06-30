import importlib
import sqlite3
from datetime import datetime


def load_module():
    pipeline = importlib.import_module('pipeline_v2')
    return importlib.reload(pipeline)


def table_exists(conn: sqlite3.Connection, table_name: str) -> bool:
    return (
        conn.execute(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?",
            (table_name,),
        ).fetchone()
        is not None
    )


def make_raw_bucket(pipeline, source_id: str, bucket_start: int) -> dict:
    bucket = pipeline.BucketAccumulator(
        source_id=source_id,
        bucket_start=bucket_start,
        bucket_end=bucket_start + pipeline.FIVE_MINUTE_SECONDS,
    )
    bucket.add_flow(
        ip_version=4,
        src_ip='192.0.2.1',
        dst_ip='198.51.100.1',
        protocol=6,
        packets=10,
        bytes_count=1000,
    )
    return bucket.raw_bucket_row()


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
    pipeline.init_stats_v3_tables(conn)
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
