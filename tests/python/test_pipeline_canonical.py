import importlib
import sqlite3


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
        ORDER BY ip_version
        """
    ).fetchall()
    assert traffic_rows == [
        ('oh_ir1_gw', '5m', 1744700700, 4, 'all', 'all', 0),
        ('oh_ir1_gw', '5m', 1744700700, 6, 'all', 'all', 0),
    ]

    address_count_rows = conn.execute(
        """
        SELECT ip_version, src_visibility, dst_visibility, address_side, unique_address_count
        FROM address_count_stats
        WHERE granularity = '5m'
        ORDER BY ip_version, address_side
        """
    ).fetchall()
    assert address_count_rows == [
        (4, 'all', 'all', 'destination', 0),
        (4, 'all', 'all', 'source', 0),
        (6, 'all', 'all', 'destination', 0),
        (6, 'all', 'all', 'source', 0),
    ]

    processed = conn.execute(
        'SELECT input_kind, source_id, bucket_start, status FROM processed_inputs'
    ).fetchone()
    assert processed == ('nfcapd', 'oh_ir1_gw', 1744700700, 'processed')
