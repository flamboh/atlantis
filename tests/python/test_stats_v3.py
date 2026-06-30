import importlib
import sqlite3


def load_modules():
    stats_v3 = importlib.import_module('stats_v3')
    maad_v2 = importlib.import_module('maad_v2')
    return importlib.reload(stats_v3), importlib.reload(maad_v2)


def test_visibility_pair_from_tos_uses_low_bits_only() -> None:
    stats_v3, _ = load_modules()

    assert stats_v3.visibility_pair_from_tos(0) == ('literal', 'literal')
    assert stats_v3.visibility_pair_from_tos(1) == ('literal', 'anonymized')
    assert stats_v3.visibility_pair_from_tos(2) == ('anonymized', 'literal')
    assert stats_v3.visibility_pair_from_tos(3) == ('anonymized', 'anonymized')
    assert stats_v3.visibility_pair_from_tos(32) == ('literal', 'literal')
    assert stats_v3.visibility_pair_from_tos(34) == ('anonymized', 'literal')


def test_merge_v3_rows_preserves_all_and_exact_scopes() -> None:
    stats_v3, _ = load_modules()
    row = stats_v3.empty_traffic_stats_v3_row(
        source_id='oh_ir1_gw',
        granularity='5m',
        bucket_start=1744733100,
        bucket_end=1744733400,
        ip_version=4,
        src_visibility='all',
        dst_visibility='all',
    )
    stats_v3.add_traffic_metrics_v3(
        row,
        protocol=6,
        flows=2,
        packets=10,
        bytes_count=1000,
    )

    merged = stats_v3.merge_traffic_rows([row, row])

    assert merged[0]['flows'] == 4
    assert merged[0]['flows_tcp'] == 4
    assert merged[0]['packets'] == 20
    assert merged[0]['bytes'] == 2000


def test_stats_v3_insert_round_trip() -> None:
    stats_v3, maad_v2 = load_modules()
    conn = sqlite3.connect(':memory:')
    stats_v3.init_stats_v3_tables(conn)
    traffic_row = stats_v3.empty_traffic_stats_v3_row(
        source_id='oh_ir1_gw',
        granularity='5m',
        bucket_start=1744733100,
        bucket_end=1744733400,
        ip_version=4,
        src_visibility='literal',
        dst_visibility='anonymized',
    )
    stats_v3.add_traffic_metrics_v3(
        traffic_row,
        protocol=17,
        flows=1,
        packets=5,
        bytes_count=500,
    )
    protocol_rows = stats_v3.protocol_set_entries_to_rows(
        [
            {
                'source_id': 'oh_ir1_gw',
                'granularity': '5m',
                'bucket_start': 1744733100,
                'bucket_end': 1744733400,
                'ip_version': 4,
                'src_visibility': 'literal',
                'dst_visibility': 'anonymized',
                'protocols': ['17'],
            }
        ]
    )
    address_rows = stats_v3.address_set_entries_to_count_rows(
        [
            {
                'source_id': 'oh_ir1_gw',
                'granularity': '5m',
                'bucket_start': 1744733100,
                'bucket_end': 1744733400,
                'ip_version': 4,
                'src_visibility': 'literal',
                'dst_visibility': 'anonymized',
                'address_side': 'destination',
                'addresses': ['198.51.100.1', '198.51.100.2'],
            }
        ]
    )
    structure_rows = stats_v3.build_address_structure_stats_v3_rows(
        source_id='oh_ir1_gw',
        granularity='5m',
        bucket_start=1744733100,
        bucket_end=1744733400,
        ip_version=4,
        src_visibility='literal',
        dst_visibility='anonymized',
        address_side='destination',
        result=maad_v2.empty_maad_result(2),
    )

    stats_v3.insert_traffic_stats_v3_rows(conn, [traffic_row])
    stats_v3.insert_protocol_stats_v3_rows(conn, protocol_rows)
    stats_v3.insert_address_count_stats_v3_rows(conn, address_rows)
    stats_v3.insert_address_structure_stats_v3_rows(conn, structure_rows)

    assert conn.execute('SELECT flows_udp, packets_udp FROM traffic_stats_v3').fetchone() == (1, 5)
    assert conn.execute('SELECT unique_protocols_count, protocols_list FROM protocol_stats_v3').fetchone() == (1, '17')
    assert conn.execute('SELECT unique_address_count FROM address_count_stats_v3').fetchone() == (2,)
    assert conn.execute(
        'SELECT COUNT(*) FROM address_structure_stats_v3 WHERE structure_kind IN '
        "('structure', 'spectrum', 'dimension')"
    ).fetchone() == (3,)
