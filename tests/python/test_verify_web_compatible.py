import importlib
import sqlite3
from pathlib import Path

import pytest


def load_modules():
    verifier = importlib.import_module('verify_web_compatible')
    stats = importlib.import_module('stats')
    processed_inputs = importlib.import_module('processed_inputs')
    return (
        importlib.reload(verifier),
        importlib.reload(stats),
        importlib.reload(processed_inputs),
    )


def test_verify_database_accepts_minimal_canonical_rollup(tmp_path: Path) -> None:
    verifier, stats, processed_inputs = load_modules()
    db_path = tmp_path / 'canonical.sqlite'
    bucket_start = 1744700700

    with sqlite3.connect(db_path) as conn:
        stats.init_stats_tables(conn)
        processed_inputs.init_processed_inputs_table(conn)
        processed_inputs.upsert_input_bucket(
            conn,
            input_kind='nfcapd',
            input_locator='nfcapd.202504150005',
            source_id='ugr16',
            bucket_start=bucket_start,
            bucket_end=bucket_start + 300,
        )
        processed_inputs.mark_input_bucket_status(
            conn,
            input_kind='nfcapd',
            input_locator='nfcapd.202504150005',
            source_id='ugr16',
            bucket_start=bucket_start,
            status='processed',
        )
        stats.insert_traffic_stats_rows(
            conn,
            [
                traffic_row(stats, '5m', bucket_start, bucket_start + 300),
                traffic_row(stats, '1h', bucket_start, bucket_start + 3600),
            ],
        )
        stats.insert_protocol_stats_rows(
            conn,
            [
                protocol_row('5m', bucket_start, bucket_start + 300),
                protocol_row('1h', bucket_start, bucket_start + 3600),
            ],
        )
        stats.insert_address_count_stats_rows(
            conn,
            [
                address_count_row('5m', bucket_start, bucket_start + 300, 'source'),
                address_count_row('5m', bucket_start, bucket_start + 300, 'destination'),
                address_count_row('1h', bucket_start, bucket_start + 3600, 'source'),
                address_count_row('1h', bucket_start, bucket_start + 3600, 'destination'),
            ],
        )
        conn.commit()

    verifier.verify_database(
        db_path,
        source_id='ugr16',
        require_data=True,
        require_processed=True,
        require_no_raw_ip=True,
    )


def test_require_processed_rejects_processed_csv_buckets_without_terminal_scan() -> None:
    verifier, _stats, processed_inputs = load_modules()
    conn = sqlite3.connect(':memory:')
    processed_inputs.init_processed_inputs_table(conn)
    processed_inputs.upsert_input_bucket(
        conn,
        input_kind='csv',
        input_locator='/csv/fatal.csv',
        scan_locator='/csv/fatal.csv',
        source_id='ugr16',
        bucket_start=1744700700,
        bucket_end=1744701000,
    )
    processed_inputs.mark_input_bucket_status(
        conn,
        input_kind='csv',
        input_locator='/csv/fatal.csv',
        source_id='ugr16',
        bucket_start=1744700700,
        status='processed',
    )

    with pytest.raises(SystemExit, match='1 incomplete CSV scan'):
        verifier.assert_processed_inputs_complete(conn)


def traffic_row(stats, granularity: str, bucket_start: int, bucket_end: int) -> dict:
    bucket_module = importlib.import_module('statistical_bucket')
    bucket = bucket_module.StatisticalBucket(
        bucket_module.BucketKey('ugr16', granularity, bucket_start, bucket_end)
    )
    bucket.add(
        bucket_module.GroupedTrafficFact(
            ip_version=4,
            protocol=6,
            src_tos=0,
            flows=1,
            packets=10,
            bytes_count=1000,
        )
    )
    return stats.canonical_bucket_rows(bucket.finish())['traffic_rows'][0]


def protocol_row(granularity: str, bucket_start: int, bucket_end: int) -> dict:
    return {
        'source_id': 'ugr16',
        'granularity': granularity,
        'bucket_start': bucket_start,
        'bucket_end': bucket_end,
        'ip_version': 4,
        'src_visibility': 'all',
        'dst_visibility': 'all',
        'unique_protocols_count': 1,
        'protocols_list': '6',
    }


def address_count_row(
    granularity: str,
    bucket_start: int,
    bucket_end: int,
    address_side: str,
) -> dict:
    return {
        'source_id': 'ugr16',
        'granularity': granularity,
        'bucket_start': bucket_start,
        'bucket_end': bucket_end,
        'ip_version': 4,
        'src_visibility': 'all',
        'dst_visibility': 'all',
        'address_side': address_side,
        'unique_address_count': 1,
    }
