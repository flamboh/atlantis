import importlib
import sqlite3


def load_modules():
    stats = importlib.import_module('stats')
    maad = importlib.import_module('maad')
    statistical_bucket = importlib.import_module('statistical_bucket')
    return importlib.reload(stats), importlib.reload(maad), importlib.reload(statistical_bucket)


def test_stats_insert_round_trip() -> None:
    stats, maad, bucket_module = load_modules()
    conn = sqlite3.connect(':memory:')
    stats.init_stats_tables(conn)
    bucket = bucket_module.StatisticalBucket(
        bucket_module.BucketKey('oh_ir1_gw', '5m', 1744733100, 1744733400)
    )
    bucket.add(
        bucket_module.FlowObservation(
            ip_version=4,
            src_ip='192.0.2.1',
            dst_ip='198.51.100.1',
            protocol=17,
            packets=5,
            bytes_count=500,
            src_tos=1,
        )
    )
    bucket.add(
        bucket_module.ScopedAddressesFact(
            bucket_module.Scope(4, 'literal', 'anonymized'),
            'destination',
            ['198.51.100.2'],
        )
    )
    rows = stats.canonical_bucket_rows(bucket.finish())
    traffic_row = next(
        row
        for row in rows['traffic_rows']
        if row['src_visibility'] == 'literal' and row['dst_visibility'] == 'anonymized'
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
    stats.insert_protocol_stats_rows(conn, rows['protocol_rows'])
    stats.insert_address_count_stats_rows(conn, rows['address_count_rows'])
    stats.insert_address_structure_stats_rows(conn, structure_rows)

    assert conn.execute('SELECT flows_udp, packets_udp FROM traffic_stats').fetchone() == (1, 5)
    assert conn.execute('SELECT unique_protocols_count, protocols_list FROM protocol_stats').fetchone() == (1, '17')
    assert conn.execute(
        "SELECT unique_address_count FROM address_count_stats "
        "WHERE src_visibility = 'literal' AND dst_visibility = 'anonymized' "
        "AND address_side = 'destination'"
    ).fetchone() == (2,)
    assert conn.execute(
        'SELECT COUNT(*) FROM address_structure_stats WHERE structure_kind IN '
        "('structure', 'spectrum', 'dimension')"
    ).fetchone() == (3,)
