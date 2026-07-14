#!/usr/bin/env python3
"""Verify a canonical pipeline SQLite database against apps/web query assumptions."""

from __future__ import annotations

import argparse
import re
import sqlite3
from pathlib import Path

from web_compat_queries import (
    assert_file_details_query,
    assert_ip_stats_query,
    assert_netflow_stats_query,
    assert_protocol_stats_query,
    assert_spectrum_stats_query,
    assert_structure_stats_query,
    select_query_window,
)


REQUIRED_COLUMNS = {
    'processed_inputs': [
        'input_kind',
        'input_locator',
        'scan_locator',
        'source_id',
        'bucket_start',
        'bucket_end',
        'status',
        'error_message',
        'discovered_at',
        'processed_at',
    ],
    'processed_input_scans': [
        'input_kind',
        'input_locator',
        'status',
        'rejected_rows',
        'skipped_bad_column_count',
        'processed_at',
    ],
    'traffic_stats': [
        'source_id',
        'granularity',
        'bucket_start',
        'bucket_end',
        'ip_version',
        'src_visibility',
        'dst_visibility',
        'flows',
        'flows_tcp',
        'flows_udp',
        'flows_icmp',
        'flows_other',
        'packets',
        'packets_tcp',
        'packets_udp',
        'packets_icmp',
        'packets_other',
        'bytes',
        'bytes_tcp',
        'bytes_udp',
        'bytes_icmp',
        'bytes_other',
        'processed_at',
    ],
    'protocol_stats': [
        'source_id',
        'granularity',
        'bucket_start',
        'bucket_end',
        'ip_version',
        'src_visibility',
        'dst_visibility',
        'unique_protocols_count',
        'protocols_list',
        'processed_at',
    ],
    'address_count_stats': [
        'source_id',
        'granularity',
        'bucket_start',
        'bucket_end',
        'ip_version',
        'src_visibility',
        'dst_visibility',
        'address_side',
        'unique_address_count',
        'processed_at',
    ],
    'address_structure_stats': [
        'source_id',
        'granularity',
        'bucket_start',
        'bucket_end',
        'ip_version',
        'src_visibility',
        'dst_visibility',
        'address_side',
        'structure_kind',
        'values_json',
        'metadata_json',
        'processed_at',
    ],
}

LEGACY_TABLES = (
    'netflow_stats_v2',
    'ip_stats_v2',
    'protocol_stats_v2',
    'structure_stats_v2',
    'spectrum_stats_v2',
    'dimension_stats_v2',
    'processed_inputs_v2',
)
IPV4_LITERAL_RE = re.compile(
    r'(?<![\d.])'
    r'(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)'
    r'(?:\.(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3}'
    r'(?![\d.])'
)
RAW_IP_COLUMN_NAMES = {
    'address',
    'client_ip',
    'da_ip',
    'destination_address',
    'destination_ip',
    'dst_addr',
    'dst_ip',
    'ip',
    'ip_addr',
    'ip_address',
    'sa_ip',
    'server_ip',
    'source_address',
    'source_ip',
    'src_addr',
    'src_ip',
}


def main() -> None:
    parser = argparse.ArgumentParser(description='Verify web-compatible canonical SQLite.')
    parser.add_argument('db_path', type=Path)
    parser.add_argument('--source-id', default=None)
    parser.add_argument('--require-data', action='store_true')
    parser.add_argument('--require-maad-data', action='store_true')
    parser.add_argument('--require-processed', action='store_true')
    parser.add_argument('--require-rollup-parity', action='store_true')
    parser.add_argument('--require-no-raw-ip', action='store_true')
    args = parser.parse_args()

    verify_database(
        args.db_path,
        source_id=args.source_id,
        require_data=args.require_data,
        require_maad_data=args.require_maad_data,
        require_processed=args.require_processed,
        require_rollup_parity=args.require_rollup_parity,
        require_no_raw_ip=args.require_no_raw_ip,
    )


def verify_database(
    db_path: Path,
    *,
    source_id: str | None,
    require_data: bool,
    require_maad_data: bool = False,
    require_processed: bool = False,
    require_rollup_parity: bool = False,
    require_no_raw_ip: bool = False,
) -> None:
    if not db_path.is_file():
        raise SystemExit(f'Database not found: {db_path}')

    with sqlite3.connect(db_path) as conn:
        conn.row_factory = sqlite3.Row
        assert_schema(conn)
        if require_no_raw_ip:
            assert_no_raw_ip_persistence(conn)
        source = source_id or first_source_id(conn)
        if source is None:
            raise SystemExit('No source_id found in traffic_stats.')

        row_counts = table_row_counts(conn)
        if require_data:
            for table_name in ('traffic_stats', 'protocol_stats', 'address_count_stats'):
                if row_counts[table_name] == 0:
                    raise SystemExit(f'{table_name} has no rows.')
            rollup_count = conn.execute(
                "SELECT COUNT(*) FROM traffic_stats WHERE granularity != '5m'"
            ).fetchone()[0]
            if rollup_count == 0:
                raise SystemExit('traffic_stats has no rollup rows.')
        if require_maad_data and row_counts['address_structure_stats'] == 0:
            raise SystemExit('address_structure_stats has no rows.')

        if require_processed:
            assert_processed_inputs_complete(conn)

        if require_processed or require_rollup_parity:
            assert_traffic_rollup_parity(conn)

        bucket_start, bucket_end = select_query_window(conn, source)
        assert_netflow_stats_query(conn, source, bucket_start, bucket_end)
        assert_ip_stats_query(conn, source, bucket_start, bucket_end)
        assert_protocol_stats_query(conn, source, bucket_start, bucket_end)
        assert_structure_stats_query(conn, source, bucket_start, bucket_end, require_maad_data)
        assert_spectrum_stats_query(conn, source, bucket_start, bucket_end, require_maad_data)
        assert_file_details_query(conn, bucket_start)

    print(f'OK {db_path}')
    print(f'source_id={source}')
    print(f'window={bucket_start}..{bucket_end}')
    for table_name, count in row_counts.items():
        print(f'{table_name}={count}')


def assert_processed_inputs_complete(conn: sqlite3.Connection) -> None:
    """Reject unfinished buckets and CSV scans lacking terminal completion."""
    pending_count = conn.execute(
        "SELECT COUNT(*) FROM processed_inputs WHERE status != 'processed'"
    ).fetchone()[0]
    if pending_count:
        raise SystemExit(f'processed_inputs has {pending_count} unprocessed rows.')
    incomplete_csv_scans = conn.execute(
        """
        SELECT COUNT(DISTINCT buckets.scan_locator)
        FROM processed_inputs AS buckets
        LEFT JOIN processed_input_scans AS scans
          ON scans.input_kind = buckets.input_kind
         AND scans.input_locator = buckets.scan_locator
         AND scans.status = 'processed'
        WHERE buckets.input_kind = 'csv'
          AND scans.input_locator IS NULL
        """
    ).fetchone()[0]
    if incomplete_csv_scans:
        raise SystemExit(
            f'processed_inputs has {incomplete_csv_scans} incomplete CSV scan(s).'
        )


def assert_schema(conn: sqlite3.Connection) -> None:
    tables = {
        row['name']
        for row in conn.execute("SELECT name FROM sqlite_master WHERE type = 'table'").fetchall()
    }
    missing_tables = sorted(set(REQUIRED_COLUMNS) - tables)
    if missing_tables:
        raise SystemExit(f'Missing canonical tables: {", ".join(missing_tables)}')

    legacy_tables = sorted(set(LEGACY_TABLES) & tables)
    if legacy_tables:
        raise SystemExit(f'Legacy tables still present: {", ".join(legacy_tables)}')

    for table_name, required_columns in REQUIRED_COLUMNS.items():
        columns = {
            row['name']
            for row in conn.execute(f'PRAGMA table_info({quote_identifier(table_name)})').fetchall()
        }
        missing_columns = sorted(set(required_columns) - columns)
        if missing_columns:
            raise SystemExit(f'{table_name} missing columns: {", ".join(missing_columns)}')


def assert_no_raw_ip_persistence(conn: sqlite3.Connection) -> None:
    """Fail when web-facing canonical tables persist raw IPv4 addresses."""
    for table_name in REQUIRED_COLUMNS:
        for column in table_columns(conn, table_name):
            if column['name'].lower() in RAW_IP_COLUMN_NAMES:
                raise SystemExit(
                    f'{table_name}.{column["name"]} looks like a raw IP address column.'
                )
            if is_text_column(column):
                assert_text_column_has_no_ipv4_literals(conn, table_name, column['name'])


def table_columns(conn: sqlite3.Connection, table_name: str) -> list[sqlite3.Row]:
    return conn.execute(f'PRAGMA table_info({quote_identifier(table_name)})').fetchall()


def is_text_column(column: sqlite3.Row) -> bool:
    column_type = str(column['type'] or '').upper()
    return 'TEXT' in column_type


def assert_text_column_has_no_ipv4_literals(
    conn: sqlite3.Connection,
    table_name: str,
    column_name: str,
) -> None:
    sql = f'SELECT {quote_identifier(column_name)} FROM {quote_identifier(table_name)}'
    for row in conn.execute(sql):
        value = row[0]
        if value is None:
            continue
        match = IPV4_LITERAL_RE.search(str(value))
        if match:
            raise SystemExit(
                f'{table_name}.{column_name} contains raw IPv4 literal {match.group(0)}.'
            )


def quote_identifier(identifier: str) -> str:
    return '"' + identifier.replace('"', '""') + '"'


def first_source_id(conn: sqlite3.Connection) -> str | None:
    row = conn.execute(
        "SELECT source_id FROM traffic_stats WHERE granularity = '5m' ORDER BY source_id LIMIT 1"
    ).fetchone()
    return None if row is None else row['source_id']


def table_row_counts(conn: sqlite3.Connection) -> dict[str, int]:
    return {
        table_name: conn.execute(f'SELECT COUNT(*) FROM {quote_identifier(table_name)}').fetchone()[
            0
        ]
        for table_name in REQUIRED_COLUMNS
    }


def assert_traffic_rollup_parity(conn: sqlite3.Connection) -> None:
    """Fail when stored traffic rollups differ from raw 5m rows."""
    mismatch_rows = conn.execute(
        """
        WITH expected AS (
            SELECT
                calendar.source_id,
                calendar.granularity,
                calendar.bucket_start,
                calendar.bucket_end,
                ts.ip_version,
                ts.src_visibility,
                ts.dst_visibility,
                SUM(ts.flows) AS flows,
                SUM(ts.flows_tcp) AS flows_tcp,
                SUM(ts.flows_udp) AS flows_udp,
                SUM(ts.flows_icmp) AS flows_icmp,
                SUM(ts.flows_other) AS flows_other,
                SUM(ts.packets) AS packets,
                SUM(ts.packets_tcp) AS packets_tcp,
                SUM(ts.packets_udp) AS packets_udp,
                SUM(ts.packets_icmp) AS packets_icmp,
                SUM(ts.packets_other) AS packets_other,
                SUM(ts.bytes) AS bytes,
                SUM(ts.bytes_tcp) AS bytes_tcp,
                SUM(ts.bytes_udp) AS bytes_udp,
                SUM(ts.bytes_icmp) AS bytes_icmp,
                SUM(ts.bytes_other) AS bytes_other
            FROM (
                SELECT source_id, granularity, bucket_start, bucket_end
                FROM address_count_stats
                WHERE granularity IN ('30m', '1h', '1d')
                GROUP BY source_id, granularity, bucket_start, bucket_end
            ) AS calendar
            JOIN traffic_stats AS ts
              ON ts.source_id = calendar.source_id
             AND ts.bucket_start >= calendar.bucket_start
             AND ts.bucket_start < calendar.bucket_end
             AND ts.granularity = '5m'
            GROUP BY
                calendar.source_id, calendar.granularity, calendar.bucket_start, calendar.bucket_end,
                ts.ip_version, ts.src_visibility, ts.dst_visibility
        ),
        actual AS (
            SELECT
                source_id,
                granularity,
                bucket_start,
                bucket_end,
                ip_version,
                src_visibility,
                dst_visibility,
                flows,
                flows_tcp,
                flows_udp,
                flows_icmp,
                flows_other,
                packets,
                packets_tcp,
                packets_udp,
                packets_icmp,
                packets_other,
                bytes,
                bytes_tcp,
                bytes_udp,
                bytes_icmp,
                bytes_other
            FROM traffic_stats
            WHERE granularity IN ('30m', '1h', '1d')
        ),
        missing_or_changed AS (
            SELECT * FROM expected
            EXCEPT
            SELECT * FROM actual
        ),
        extra_or_changed AS (
            SELECT * FROM actual
            EXCEPT
            SELECT * FROM expected
        )
        SELECT 'missing_or_changed' AS kind, COUNT(*) AS count FROM missing_or_changed
        UNION ALL
        SELECT 'extra_or_changed' AS kind, COUNT(*) AS count FROM extra_or_changed
        """
    ).fetchall()
    mismatches = {row['kind']: row['count'] for row in mismatch_rows}
    total_mismatches = sum(mismatches.values())
    if total_mismatches:
        raise SystemExit(
            'traffic_stats rollup parity failed: '
            f"missing_or_changed={mismatches.get('missing_or_changed', 0)}, "
            f"extra_or_changed={mismatches.get('extra_or_changed', 0)}"
        )
if __name__ == '__main__':
    main()
