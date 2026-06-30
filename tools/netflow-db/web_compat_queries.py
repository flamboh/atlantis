"""SQLite query checks that mirror apps/web canonical database routes."""

from __future__ import annotations

import sqlite3
from datetime import datetime
from zoneinfo import ZoneInfo


def select_query_window(conn: sqlite3.Connection, source_id: str) -> tuple[int, int]:
    row = conn.execute(
        """
        SELECT MIN(bucket_start) AS start, MAX(bucket_end) AS end
        FROM traffic_stats
        WHERE source_id = ?
          AND granularity = '5m'
          AND src_visibility = 'all'
          AND dst_visibility = 'all'
        """,
        (source_id,),
    ).fetchone()
    if row['start'] is None or row['end'] is None:
        raise SystemExit(f'No traffic rows found for source_id={source_id}.')
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
        FROM traffic_stats
        WHERE source_id IN (?)
          AND granularity = '1h'
          AND src_visibility = 'all'
          AND dst_visibility = 'all'
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
               SUM(CASE WHEN address_side = 'source' AND ip_version = 4 THEN unique_address_count ELSE 0 END) AS saIpv4Count,
               SUM(CASE WHEN address_side = 'destination' AND ip_version = 4 THEN unique_address_count ELSE 0 END) AS daIpv4Count,
               SUM(CASE WHEN address_side = 'source' AND ip_version = 6 THEN unique_address_count ELSE 0 END) AS saIpv6Count,
               SUM(CASE WHEN address_side = 'destination' AND ip_version = 6 THEN unique_address_count ELSE 0 END) AS daIpv6Count,
               MAX(processed_at) AS processedAt
        FROM address_count_stats
        WHERE granularity = '1h'
          AND source_id IN (?)
          AND src_visibility = 'all'
          AND dst_visibility = 'all'
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
               SUM(CASE WHEN ip_version = 4 THEN unique_protocols_count ELSE 0 END) AS uniqueProtocolsIpv4,
               SUM(CASE WHEN ip_version = 6 THEN unique_protocols_count ELSE 0 END) AS uniqueProtocolsIpv6,
               MAX(processed_at) AS processedAt
        FROM protocol_stats
        WHERE granularity = '1h'
          AND source_id IN (?)
          AND src_visibility = 'all'
          AND dst_visibility = 'all'
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
               MAX(CASE WHEN address_side = 'source' THEN values_json END) AS structureJsonSa,
               MAX(CASE WHEN address_side = 'destination' THEN values_json END) AS structureJsonDa
        FROM address_structure_stats
        WHERE granularity = '1h'
          AND source_id IN (?)
          AND bucket_start >= ?
          AND bucket_start < ?
          AND ip_version = 4
          AND src_visibility = 'all'
          AND dst_visibility = 'all'
          AND structure_kind = 'structure'
        GROUP BY source_id, bucket_start
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
               MAX(CASE WHEN address_side = 'source' THEN values_json END) AS spectrumJsonSa,
               MAX(CASE WHEN address_side = 'destination' THEN values_json END) AS spectrumJsonDa
        FROM address_structure_stats
        WHERE granularity = '1h'
          AND source_id IN (?)
          AND bucket_start >= ?
          AND bucket_start < ?
          AND ip_version = 4
          AND src_visibility = 'all'
          AND dst_visibility = 'all'
          AND structure_kind = 'spectrum'
        GROUP BY source_id, bucket_start
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
               ip.saIpv4Count AS saIpv4Count
        FROM (
            SELECT source_id AS router,
                   bucket_start,
                   MAX(bucket_end) AS bucket_end,
                   SUM(flows) AS flows,
                   MAX(processed_at) AS processed_at
            FROM traffic_stats
            WHERE granularity = '5m'
              AND bucket_start = ?
              AND src_visibility = 'all'
              AND dst_visibility = 'all'
            GROUP BY source_id, bucket_start
        ) ns
        LEFT JOIN (
            SELECT source_id,
                   bucket_start,
                   MIN(input_locator) AS input_locator
            FROM processed_inputs
            WHERE bucket_start = ?
            GROUP BY source_id, bucket_start
        ) pi
            ON pi.source_id = ns.router
           AND pi.bucket_start = ns.bucket_start
        LEFT JOIN (
            SELECT source_id,
                   bucket_start,
                   SUM(CASE WHEN address_side = 'source' AND ip_version = 4 THEN unique_address_count ELSE 0 END) AS saIpv4Count
            FROM address_count_stats
            WHERE granularity = '5m'
              AND bucket_start = ?
              AND src_visibility = 'all'
              AND dst_visibility = 'all'
            GROUP BY source_id, bucket_start
        ) ip
            ON ip.source_id = ns.router
           AND ip.bucket_start = ns.bucket_start
        ORDER BY ns.router
        LIMIT 1
        """,
        (parsed_bucket_start, parsed_bucket_start, parsed_bucket_start),
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
