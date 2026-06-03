#!/usr/bin/env python3
"""Verify a pipeline v2 SQLite database against apps/web query assumptions."""

from __future__ import annotations

import argparse
import re
import sqlite3
from datetime import datetime
from pathlib import Path
from zoneinfo import ZoneInfo


REQUIRED_COLUMNS = {
    'processed_inputs_v2': [
        'input_kind',
        'input_locator',
        'source_id',
        'bucket_start',
        'bucket_end',
        'status',
        'error_message',
        'discovered_at',
        'processed_at',
    ],
    'netflow_stats_v2': [
        'source_id',
        'granularity',
        'bucket_start',
        'bucket_end',
        'ip_version',
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
    'ip_stats_v2': [
        'source_id',
        'granularity',
        'bucket_start',
        'bucket_end',
        'sa_ipv4_count',
        'da_ipv4_count',
        'sa_ipv6_count',
        'da_ipv6_count',
        'processed_at',
    ],
    'protocol_stats_v2': [
        'source_id',
        'granularity',
        'bucket_start',
        'bucket_end',
        'unique_protocols_count_ipv4',
        'unique_protocols_count_ipv6',
        'protocols_list_ipv4',
        'protocols_list_ipv6',
        'processed_at',
    ],
    'structure_stats_v2': [
        'source_id',
        'granularity',
        'bucket_start',
        'bucket_end',
        'ip_version',
        'structure_json_sa',
        'structure_json_da',
        'metadata_json_sa',
        'metadata_json_da',
        'processed_at',
    ],
    'spectrum_stats_v2': [
        'source_id',
        'granularity',
        'bucket_start',
        'bucket_end',
        'ip_version',
        'spectrum_json_sa',
        'spectrum_json_da',
        'metadata_json_sa',
        'metadata_json_da',
        'processed_at',
    ],
    'dimension_stats_v2': [
        'source_id',
        'granularity',
        'bucket_start',
        'bucket_end',
        'ip_version',
        'dimensions_json_sa',
        'dimensions_json_da',
        'metadata_json_sa',
        'metadata_json_da',
        'processed_at',
    ],
}

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
    parser = argparse.ArgumentParser(description='Verify web-compatible pipeline v2 SQLite.')
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
            raise SystemExit('No source_id found in netflow_stats_v2.')

        row_counts = table_row_counts(conn)
        if require_data:
            for table_name in (
                'netflow_stats_v2',
                'ip_stats_v2',
                'protocol_stats_v2',
            ):
                if row_counts[table_name] == 0:
                    raise SystemExit(f'{table_name} has no rows.')
            netflow_rollup_count = conn.execute(
                "SELECT COUNT(*) FROM netflow_stats_v2 WHERE granularity != '5m'"
            ).fetchone()[0]
            if netflow_rollup_count == 0:
                raise SystemExit('netflow_stats_v2 has no rollup rows.')
        if require_maad_data:
            for table_name in ('structure_stats_v2', 'spectrum_stats_v2', 'dimension_stats_v2'):
                if row_counts[table_name] == 0:
                    raise SystemExit(f'{table_name} has no rows.')

        if require_processed:
            pending_count = conn.execute(
                "SELECT COUNT(*) FROM processed_inputs_v2 WHERE status != 'processed'"
            ).fetchone()[0]
            if pending_count:
                raise SystemExit(f'processed_inputs_v2 has {pending_count} unprocessed rows.')

        if require_processed or require_rollup_parity:
            assert_netflow_rollup_parity(conn)

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


def assert_schema(conn: sqlite3.Connection) -> None:
    tables = {
        row['name']
        for row in conn.execute("SELECT name FROM sqlite_master WHERE type = 'table'").fetchall()
    }
    missing_tables = sorted(set(REQUIRED_COLUMNS) - tables)
    if missing_tables:
        raise SystemExit(f'Missing v2 tables: {", ".join(missing_tables)}')

    for table_name, required_columns in REQUIRED_COLUMNS.items():
        columns = {
            row['name']
            for row in conn.execute(f'PRAGMA table_info({table_name})').fetchall()
        }
        missing_columns = sorted(set(required_columns) - columns)
        if missing_columns:
            raise SystemExit(
                f'{table_name} missing columns: {", ".join(missing_columns)}'
            )


def assert_no_raw_ip_persistence(conn: sqlite3.Connection) -> None:
    """Fail when web-facing v2 tables persist raw IPv4 addresses."""
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
        "SELECT source_id FROM netflow_stats_v2 WHERE granularity = '5m' ORDER BY source_id LIMIT 1"
    ).fetchone()
    return None if row is None else row['source_id']


def table_row_counts(conn: sqlite3.Connection) -> dict[str, int]:
    return {
        table_name: conn.execute(f'SELECT COUNT(*) FROM {table_name}').fetchone()[0]
        for table_name in REQUIRED_COLUMNS
    }


def select_query_window(conn: sqlite3.Connection, source_id: str) -> tuple[int, int]:
    row = conn.execute(
        """
        SELECT MIN(bucket_start) AS start, MAX(bucket_end) AS end
        FROM netflow_stats_v2
        WHERE source_id = ?
          AND granularity = '5m'
        """,
        (source_id,),
    ).fetchone()
    if row['start'] is None or row['end'] is None:
        raise SystemExit(f'No netflow rows found for source_id={source_id}.')
    return row['start'], row['end']


def assert_netflow_stats_query(
    conn: sqlite3.Connection,
    source_id: str,
    bucket_start: int,
    bucket_end: int,
) -> None:
    row = conn.execute(
        """
        SELECT bucket_start AS bucketStart,
               SUM(flows) AS flows,
               SUM(packets) AS packets,
               SUM(bytes) AS bytes,
               SUM(CASE WHEN ip_version = 4 THEN flows ELSE 0 END) AS flowsIpv4,
               SUM(CASE WHEN ip_version = 6 THEN flows ELSE 0 END) AS flowsIpv6
        FROM netflow_stats_v2
        WHERE source_id IN (?)
          AND granularity = '1h'
          AND bucket_start >= ?
          AND bucket_start < ?
        GROUP BY bucketStart
        ORDER BY bucketStart
        LIMIT 1
        """,
        (source_id, bucket_start, bucket_end),
    ).fetchone()
    if row is None or row['flows'] is None:
        raise SystemExit('Web netflow stats query returned no rows.')


def assert_netflow_rollup_parity(conn: sqlite3.Connection) -> None:
    """Fail when stored netflow rollups differ from raw 5m rows."""
    mismatch_rows = conn.execute(
        """
        WITH expected AS (
            SELECT
                calendar.source_id,
                calendar.granularity,
                calendar.bucket_start,
                calendar.bucket_end,
                ns.ip_version,
                SUM(ns.flows) AS flows,
                SUM(ns.flows_tcp) AS flows_tcp,
                SUM(ns.flows_udp) AS flows_udp,
                SUM(ns.flows_icmp) AS flows_icmp,
                SUM(ns.flows_other) AS flows_other,
                SUM(ns.packets) AS packets,
                SUM(ns.packets_tcp) AS packets_tcp,
                SUM(ns.packets_udp) AS packets_udp,
                SUM(ns.packets_icmp) AS packets_icmp,
                SUM(ns.packets_other) AS packets_other,
                SUM(ns.bytes) AS bytes,
                SUM(ns.bytes_tcp) AS bytes_tcp,
                SUM(ns.bytes_udp) AS bytes_udp,
                SUM(ns.bytes_icmp) AS bytes_icmp,
                SUM(ns.bytes_other) AS bytes_other
            FROM (
                SELECT source_id, granularity, bucket_start, bucket_end
                FROM ip_stats_v2
                WHERE granularity IN ('30m', '1h', '1d')
            ) AS calendar
            JOIN netflow_stats_v2 AS ns
              ON ns.source_id = calendar.source_id
             AND ns.bucket_start >= calendar.bucket_start
             AND ns.bucket_start < calendar.bucket_end
             AND ns.granularity = '5m'
            GROUP BY calendar.source_id, calendar.granularity, calendar.bucket_start, calendar.bucket_end, ns.ip_version
        ),
        actual AS (
            SELECT
                source_id,
                granularity,
                bucket_start,
                bucket_end,
                ip_version,
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
            FROM netflow_stats_v2
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
            'netflow_stats_v2 rollup parity failed: '
            f"missing_or_changed={mismatches.get('missing_or_changed', 0)}, "
            f"extra_or_changed={mismatches.get('extra_or_changed', 0)}"
        )


def assert_ip_stats_query(
    conn: sqlite3.Connection,
    source_id: str,
    bucket_start: int,
    bucket_end: int,
) -> None:
    row = conn.execute(
        """
        SELECT source_id AS router,
               bucket_start AS bucketStart,
               bucket_end AS bucketEnd,
               granularity,
               SUM(sa_ipv4_count) AS saIpv4Count,
               SUM(da_ipv4_count) AS daIpv4Count,
               SUM(sa_ipv6_count) AS saIpv6Count,
               SUM(da_ipv6_count) AS daIpv6Count,
               MAX(processed_at) AS processedAt
        FROM ip_stats_v2
        WHERE granularity = '1h'
          AND source_id IN (?)
          AND bucket_start >= ?
          AND bucket_start < ?
        GROUP BY source_id, bucket_start, bucket_end, granularity
        ORDER BY source_id ASC, bucket_start ASC
        LIMIT 1
        """,
        (source_id, bucket_start, bucket_end),
    ).fetchone()
    if row is None:
        raise SystemExit('Web IP stats query returned no rows.')


def assert_protocol_stats_query(
    conn: sqlite3.Connection,
    source_id: str,
    bucket_start: int,
    bucket_end: int,
) -> None:
    row = conn.execute(
        """
        SELECT source_id AS router,
               bucket_start AS bucketStart,
               bucket_end AS bucketEnd,
               granularity,
               SUM(unique_protocols_count_ipv4) AS uniqueProtocolsIpv4,
               SUM(unique_protocols_count_ipv6) AS uniqueProtocolsIpv6,
               MAX(processed_at) AS processedAt
        FROM protocol_stats_v2
        WHERE granularity = '1h'
          AND source_id IN (?)
          AND bucket_start >= ?
          AND bucket_start < ?
        GROUP BY source_id, bucket_start, bucket_end, granularity
        ORDER BY source_id ASC, bucket_start ASC
        LIMIT 1
        """,
        (source_id, bucket_start, bucket_end),
    ).fetchone()
    if row is None:
        raise SystemExit('Web protocol stats query returned no rows.')


def assert_structure_stats_query(
    conn: sqlite3.Connection,
    source_id: str,
    bucket_start: int,
    bucket_end: int,
    require_rows: bool,
) -> None:
    row = conn.execute(
        """
        SELECT source_id AS router,
               bucket_start AS bucketStart,
               structure_json_sa AS structureJsonSa,
               structure_json_da AS structureJsonDa
        FROM structure_stats_v2
        WHERE granularity = '1h'
          AND source_id IN (?)
          AND bucket_start >= ?
          AND bucket_start < ?
          AND ip_version = 4
        ORDER BY source_id ASC, bucket_start ASC
        LIMIT 1
        """,
        (source_id, bucket_start, bucket_end),
    ).fetchone()
    if require_rows and row is None:
        raise SystemExit('Web structure stats query returned no rows.')


def assert_spectrum_stats_query(
    conn: sqlite3.Connection,
    source_id: str,
    bucket_start: int,
    bucket_end: int,
    require_rows: bool,
) -> None:
    row = conn.execute(
        """
        SELECT source_id AS router,
               bucket_start AS bucketStart,
               spectrum_json_sa AS spectrumJsonSa,
               spectrum_json_da AS spectrumJsonDa
        FROM spectrum_stats_v2
        WHERE granularity = '1h'
          AND source_id IN (?)
          AND bucket_start >= ?
          AND bucket_start < ?
          AND ip_version = 4
        ORDER BY source_id ASC, bucket_start ASC
        LIMIT 1
        """,
        (source_id, bucket_start, bucket_end),
    ).fetchone()
    if require_rows and row is None:
        raise SystemExit('Web spectrum stats query returned no rows.')


def assert_file_details_query(conn: sqlite3.Connection, bucket_start: int) -> None:
    slug = datetime.fromtimestamp(bucket_start, ZoneInfo('America/Los_Angeles')).strftime(
        '%Y%m%d%H%M'
    )
    parsed_bucket_start = slug_to_bucket_start(slug)
    row = conn.execute(
        """
        SELECT ns.router,
               ns.bucket_start,
               ns.flows,
               pi.input_locator AS file_path,
               ip.sa_ipv4_count AS saIpv4Count
        FROM (
            SELECT source_id AS router,
                   bucket_start,
                   MAX(bucket_end) AS bucket_end,
                   SUM(flows) AS flows,
                   MAX(processed_at) AS processed_at
            FROM netflow_stats_v2
            WHERE granularity = '5m'
              AND bucket_start = ?
            GROUP BY source_id, bucket_start
        ) ns
        LEFT JOIN (
            SELECT source_id,
                   bucket_start,
                   MIN(input_locator) AS input_locator
            FROM processed_inputs_v2
            WHERE bucket_start = ?
            GROUP BY source_id, bucket_start
        ) pi
            ON pi.source_id = ns.router
           AND pi.bucket_start = ns.bucket_start
        LEFT JOIN ip_stats_v2 ip
            ON ip.source_id = ns.router
           AND ip.granularity = '5m'
           AND ip.bucket_start = ns.bucket_start
        ORDER BY ns.router
        LIMIT 1
        """,
        (parsed_bucket_start, parsed_bucket_start),
    ).fetchone()
    if row is None:
        raise SystemExit(f'Web file details query returned no rows for slug {slug}.')


def slug_to_bucket_start(slug: str) -> int:
    timezone = ZoneInfo('America/Los_Angeles')
    parsed = datetime(
        int(slug[0:4]),
        int(slug[4:6]),
        int(slug[6:8]),
        int(slug[8:10]),
        int(slug[10:12]),
        tzinfo=timezone,
    )
    return int(parsed.timestamp())


if __name__ == '__main__':
    main()
