import importlib
import sqlite3


def load_modules():
    stats = importlib.import_module('stats')
    maad = importlib.import_module('maad')
    return importlib.reload(stats), importlib.reload(maad)


def test_visibility_pair_from_tos_uses_low_bits_only() -> None:
    stats, _ = load_modules()

    assert stats.visibility_pair_from_tos(0) == ('literal', 'literal')
    assert stats.visibility_pair_from_tos(1) == ('literal', 'anonymized')
    assert stats.visibility_pair_from_tos(2) == ('anonymized', 'literal')
    assert stats.visibility_pair_from_tos(3) == ('anonymized', 'anonymized')
    assert stats.visibility_pair_from_tos(32) == ('literal', 'literal')
    assert stats.visibility_pair_from_tos(34) == ('anonymized', 'literal')


def test_merge_v3_rows_preserves_all_and_exact_scopes() -> None:
    stats, _ = load_modules()
    row = stats.empty_traffic_stats_row(
        source_id='oh_ir1_gw',
        granularity='5m',
        bucket_start=1744733100,
        bucket_end=1744733400,
        ip_version=4,
        src_visibility='all',
        dst_visibility='all',
    )
    stats.add_traffic_metrics(
        row,
        protocol=6,
        flows=2,
        packets=10,
        bytes_count=1000,
    )

    merged = stats.merge_traffic_rows([row, row])

    assert merged[0]['flows'] == 4
    assert merged[0]['flows_tcp'] == 4
    assert merged[0]['packets'] == 20
    assert merged[0]['bytes'] == 2000


def test_stats_insert_round_trip() -> None:
    stats, maad = load_modules()
    conn = sqlite3.connect(':memory:')
    stats.init_stats_tables(conn)
    traffic_row = stats.empty_traffic_stats_row(
        source_id='oh_ir1_gw',
        granularity='5m',
        bucket_start=1744733100,
        bucket_end=1744733400,
        ip_version=4,
        src_visibility='literal',
        dst_visibility='anonymized',
    )
    stats.add_traffic_metrics(
        traffic_row,
        protocol=17,
        flows=1,
        packets=5,
        bytes_count=500,
    )
    protocol_rows = stats.protocol_set_entries_to_rows(
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
    address_rows = stats.address_set_entries_to_count_rows(
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
    structure_rows = stats.build_address_structure_stats_rows(
        source_id='oh_ir1_gw',
        granularity='5m',
        bucket_start=1744733100,
        bucket_end=1744733400,
        ip_version=4,
        src_visibility='literal',
        dst_visibility='anonymized',
        address_side='destination',
        result=maad.empty_maad_result(2),
    )

    stats.insert_traffic_stats_rows(conn, [traffic_row])
    stats.insert_protocol_stats_rows(conn, protocol_rows)
    stats.insert_address_count_stats_rows(conn, address_rows)
    stats.insert_address_structure_stats_rows(conn, structure_rows)

    assert conn.execute('SELECT flows_udp, packets_udp FROM traffic_stats').fetchone() == (1, 5)
    assert conn.execute('SELECT unique_protocols_count, protocols_list FROM protocol_stats').fetchone() == (1, '17')
    assert conn.execute('SELECT unique_address_count FROM address_count_stats').fetchone() == (2,)
    assert conn.execute(
        'SELECT COUNT(*) FROM address_structure_stats WHERE structure_kind IN '
        "('structure', 'spectrum', 'dimension')"
    ).fetchone() == (3,)
