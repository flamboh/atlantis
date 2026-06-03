import importlib
import sqlite3

import pytest


def load_modules():
    processed_inputs_v2 = importlib.import_module('processed_inputs_v2')
    stats_v2 = importlib.import_module('stats_v2')
    normalized_rows_v2 = importlib.import_module('normalized_rows_v2')
    return (
        importlib.reload(processed_inputs_v2),
        importlib.reload(stats_v2),
        importlib.reload(normalized_rows_v2),
    )


def make_row(normalized_rows_v2, **overrides):
    base = {
        'source_id': 'oh_ir1_gw',
        'bucket_start': 1744733100,
        'bucket_end': 1744733400,
        'time_received': 1744733279,
        'time_end': 1744733000,
        'time_start': 1744732700,
        'src_ip': '192.0.2.1',
        'dst_ip': '198.51.100.9',
        'ip_version': 4,
        'src_port': 443,
        'dst_port': 55000,
        'protocol': 6,
        'packets': 10,
        'bytes': 2048,
        'src_tos': 2,
        'dst_tos': 0,
    }
    base.update(overrides)
    return normalized_rows_v2.NormalizedRow(**base)


def test_processed_inputs_v2_tracks_pending_and_processed_status() -> None:
    processed_inputs_v2, _, _ = load_modules()
    conn = sqlite3.connect(':memory:')
    processed_inputs_v2.init_processed_inputs_v2_table(conn)

    processed_inputs_v2.upsert_input_bucket(
        conn,
        input_kind='nfcapd',
        input_locator='/captures/oh_ir1_gw/2025/04/15/nfcapd.202504150005',
        source_id='oh_ir1_gw',
        bucket_start=1744733100,
        bucket_end=1744733400,
    )

    pending = processed_inputs_v2.get_pending_inputs(conn)
    assert pending == [
        {
            'input_kind': 'nfcapd',
            'input_locator': '/captures/oh_ir1_gw/2025/04/15/nfcapd.202504150005',
            'source_id': 'oh_ir1_gw',
            'bucket_start': 1744733100,
            'bucket_end': 1744733400,
        }
    ]

    processed_inputs_v2.mark_input_bucket_status(
        conn,
        input_kind='nfcapd',
        input_locator='/captures/oh_ir1_gw/2025/04/15/nfcapd.202504150005',
        source_id='oh_ir1_gw',
        bucket_start=1744733100,
        status='processed',
    )

    assert processed_inputs_v2.get_pending_inputs(conn) == []

    processed_inputs_v2.mark_input_bucket_status(
        conn,
        input_kind='nfcapd',
        input_locator='/captures/oh_ir1_gw/2025/04/15/nfcapd.202504150005',
        source_id='oh_ir1_gw',
        bucket_start=1744733100,
        status='failed',
        error_message='worker failed',
    )

    row = conn.execute(
        'SELECT status, error_message, processed_at FROM processed_inputs_v2'
    ).fetchone()
    assert row[0:2] == ('failed', 'worker failed')
    assert row[2] is not None
    assert processed_inputs_v2.get_pending_inputs(conn) == pending


def test_stats_v2_aggregates_rows_by_bucket_and_family() -> None:
    _, stats_v2, normalized_rows_v2 = load_modules()
    rows = [
        make_row(normalized_rows_v2, protocol=6, packets=10, bytes=1000),
        make_row(
            normalized_rows_v2,
            src_ip='192.0.2.2',
            dst_ip='198.51.100.10',
            protocol=17,
            packets=5,
            bytes=500,
        ),
        make_row(
            normalized_rows_v2,
            src_ip='2001:db8::1',
            dst_ip='2001:db8::2',
            ip_version=6,
            protocol=58,
            packets=3,
            bytes=300,
        ),
    ]

    netflow_rows = stats_v2.build_netflow_stats_v2_rows(rows)
    ip_rows = stats_v2.build_ip_stats_v2_rows(rows)
    protocol_rows = stats_v2.build_protocol_stats_v2_rows(rows)

    assert netflow_rows == [
        {
            'source_id': 'oh_ir1_gw',
            'granularity': '5m',
            'bucket_start': 1744733100,
            'bucket_end': 1744733400,
            'ip_version': 4,
            'flows': 2,
            'flows_tcp': 1,
            'flows_udp': 1,
            'flows_icmp': 0,
            'flows_other': 0,
            'packets': 15,
            'packets_tcp': 10,
            'packets_udp': 5,
            'packets_icmp': 0,
            'packets_other': 0,
            'bytes': 1500,
            'bytes_tcp': 1000,
            'bytes_udp': 500,
            'bytes_icmp': 0,
            'bytes_other': 0,
        },
        {
            'source_id': 'oh_ir1_gw',
            'granularity': '5m',
            'bucket_start': 1744733100,
            'bucket_end': 1744733400,
            'ip_version': 6,
            'flows': 1,
            'flows_tcp': 0,
            'flows_udp': 0,
            'flows_icmp': 1,
            'flows_other': 0,
            'packets': 3,
            'packets_tcp': 0,
            'packets_udp': 0,
            'packets_icmp': 3,
            'packets_other': 0,
            'bytes': 300,
            'bytes_tcp': 0,
            'bytes_udp': 0,
            'bytes_icmp': 300,
            'bytes_other': 0,
        },
    ]
    assert ip_rows == [
        {
            'source_id': 'oh_ir1_gw',
            'granularity': '5m',
            'bucket_start': 1744733100,
            'bucket_end': 1744733400,
            'sa_ipv4_count': 2,
            'da_ipv4_count': 2,
            'sa_ipv6_count': 1,
            'da_ipv6_count': 1,
        }
    ]
    assert protocol_rows == [
        {
            'source_id': 'oh_ir1_gw',
            'granularity': '5m',
            'bucket_start': 1744733100,
            'bucket_end': 1744733400,
            'unique_protocols_count_ipv4': 2,
            'unique_protocols_count_ipv6': 1,
            'protocols_list_ipv4': '17,6',
            'protocols_list_ipv6': '58',
        }
    ]


def test_stats_v2_rejects_inconsistent_bucket_bounds() -> None:
    _, stats_v2, normalized_rows_v2 = load_modules()
    rows = [
        make_row(normalized_rows_v2),
        make_row(normalized_rows_v2, bucket_end=1744733700),
    ]

    with pytest.raises(ValueError, match='Inconsistent bucket_end'):
        stats_v2.build_ip_stats_v2_rows(rows)

    with pytest.raises(ValueError, match='Inconsistent bucket_end'):
        stats_v2.build_protocol_stats_v2_rows(rows)


def test_stats_v2_rejects_invalid_ip_version() -> None:
    _, stats_v2, normalized_rows_v2 = load_modules()
    rows = [make_row(normalized_rows_v2, ip_version=5)]

    with pytest.raises(ValueError, match='Unsupported ip_version'):
        stats_v2.build_ip_stats_v2_rows(rows)

    with pytest.raises(ValueError, match='Unsupported ip_version'):
        stats_v2.build_protocol_stats_v2_rows(rows)


def test_stats_v2_insert_persists_aggregates() -> None:
    _, stats_v2, normalized_rows_v2 = load_modules()
    conn = sqlite3.connect(':memory:')
    stats_v2.init_netflow_stats_v2_table(conn)
    stats_v2.init_ip_stats_v2_table(conn)
    stats_v2.init_protocol_stats_v2_table(conn)

    rows = [
        make_row(normalized_rows_v2, protocol=6, packets=10, bytes=1000),
        make_row(
            normalized_rows_v2,
            src_ip='2001:db8::1',
            dst_ip='2001:db8::2',
            ip_version=6,
            protocol=58,
            packets=3,
            bytes=300,
        ),
    ]

    stats_v2.insert_netflow_stats_v2_rows(conn, stats_v2.build_netflow_stats_v2_rows(rows))
    stats_v2.insert_netflow_stats_v2_rows(
        conn,
        [
            {
                **row,
                'granularity': '30m',
                'bucket_start': 1744732800,
                'bucket_end': 1744734600,
            }
            for row in stats_v2.build_netflow_stats_v2_rows(rows)
        ],
    )
    stats_v2.insert_ip_stats_v2_rows(conn, stats_v2.build_ip_stats_v2_rows(rows))
    stats_v2.insert_protocol_stats_v2_rows(conn, stats_v2.build_protocol_stats_v2_rows(rows))

    netflow = conn.execute(
        "SELECT source_id, granularity, bucket_start, ip_version, flows, packets, bytes FROM netflow_stats_v2 WHERE granularity = '5m' ORDER BY ip_version"
    ).fetchall()
    ip_stats = conn.execute(
        'SELECT source_id, granularity, bucket_start, sa_ipv4_count, da_ipv4_count, sa_ipv6_count, da_ipv6_count FROM ip_stats_v2'
    ).fetchall()
    netflow_aggregates = conn.execute(
        "SELECT source_id, granularity, bucket_start, ip_version, flows, packets, bytes FROM netflow_stats_v2 WHERE granularity = '30m' ORDER BY ip_version"
    ).fetchall()
    protocol_stats = conn.execute(
        'SELECT source_id, granularity, bucket_start, unique_protocols_count_ipv4, unique_protocols_count_ipv6, protocols_list_ipv4, protocols_list_ipv6 FROM protocol_stats_v2'
    ).fetchall()

    assert netflow == [
        ('oh_ir1_gw', '5m', 1744733100, 4, 1, 10, 1000),
        ('oh_ir1_gw', '5m', 1744733100, 6, 1, 3, 300),
    ]
    assert ip_stats == [('oh_ir1_gw', '5m', 1744733100, 1, 1, 1, 1)]
    assert netflow_aggregates == [
        ('oh_ir1_gw', '30m', 1744732800, 4, 1, 10, 1000),
        ('oh_ir1_gw', '30m', 1744732800, 6, 1, 3, 300),
    ]
    assert protocol_stats == [('oh_ir1_gw', '5m', 1744733100, 1, 1, '6', '58')]

    index_names = {
        row[1]
        for row in conn.execute("PRAGMA index_list('netflow_stats_v2')").fetchall()
    }
    assert 'idx_netflow_stats_v2_granularity_bucket_source' in index_names
