import importlib
import sqlite3
from pathlib import Path


def load_modules():
    verifier = importlib.import_module('verify_web_compatible')
    stats_v3 = importlib.import_module('stats_v3')
    processed_inputs = importlib.import_module('processed_inputs')
    return (
        importlib.reload(verifier),
        importlib.reload(stats_v3),
        importlib.reload(processed_inputs),
    )


def test_verify_database_accepts_minimal_canonical_rollup(tmp_path: Path) -> None:
    verifier, stats_v3, processed_inputs = load_modules()
    db_path = tmp_path / 'canonical.sqlite'
    bucket_start = 1744700700

    with sqlite3.connect(db_path) as conn:
        stats_v3.init_stats_v3_tables(conn)
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
        stats_v3.insert_traffic_stats_rows(
            conn,
            [
                traffic_row(stats_v3, '5m', bucket_start, bucket_start + 300),
                traffic_row(stats_v3, '1h', bucket_start, bucket_start + 3600),
            ],
        )
        stats_v3.insert_protocol_stats_rows(
            conn,
            [
                protocol_row('5m', bucket_start, bucket_start + 300),
                protocol_row('1h', bucket_start, bucket_start + 3600),
            ],
        )
        stats_v3.insert_address_count_stats_rows(
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


def traffic_row(stats_v3, granularity: str, bucket_start: int, bucket_end: int) -> dict:
    row = stats_v3.empty_traffic_stats_row(
        source_id='ugr16',
        granularity=granularity,
        bucket_start=bucket_start,
        bucket_end=bucket_end,
        ip_version=4,
        src_visibility='all',
        dst_visibility='all',
    )
    stats_v3.add_traffic_metrics_v3(row, protocol=6, flows=1, packets=10, bytes_count=1000)
    return row


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
