"""Dimensioned aggregate tables with address visibility scope."""

from __future__ import annotations

import json
import sqlite3
from typing import Iterable

from maad import MaadJsonResult
from statistical_bucket import CanonicalBucket


STRUCTURE_KINDS = ('structure', 'spectrum', 'dimension')
NETFLOW_METRIC_COLUMNS = (
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
)


def canonical_bucket_rows(bucket: CanonicalBucket) -> dict[str, list[dict]]:
    """Render an immutable canonical bucket for the SQLite and MAAD adapters."""
    base = {
        'source_id': bucket.key.source_id,
        'granularity': bucket.key.granularity,
        'bucket_start': bucket.key.bucket_start,
        'bucket_end': bucket.key.bucket_end,
    }
    traffic_rows = [
        {
            **base,
            'ip_version': entry.scope.ip_version,
            'src_visibility': entry.scope.src_visibility,
            'dst_visibility': entry.scope.dst_visibility,
            **{
                column: getattr(entry.metrics, column)
                for column in NETFLOW_METRIC_COLUMNS
            },
        }
        for entry in bucket.traffic
    ]
    protocol_rows = [
        {
            **base,
            'ip_version': entry.scope.ip_version,
            'src_visibility': entry.scope.src_visibility,
            'dst_visibility': entry.scope.dst_visibility,
            'unique_protocols_count': len(entry.protocols),
            'protocols_list': ','.join(entry.protocols),
        }
        for entry in bucket.protocols
    ]
    address_sets = [
        {
            **base,
            'ip_version': entry.scope.ip_version,
            'src_visibility': entry.scope.src_visibility,
            'dst_visibility': entry.scope.dst_visibility,
            'address_side': entry.address_side,
            'addresses': list(entry.addresses),
        }
        for entry in bucket.addresses
    ]
    address_count_rows = [
        {
            **{key: value for key, value in entry.items() if key != 'addresses'},
            'unique_address_count': len(entry['addresses']),
        }
        for entry in address_sets
    ]
    return {
        'traffic_rows': traffic_rows,
        'protocol_rows': protocol_rows,
        'address_count_rows': address_count_rows,
        'address_sets': address_sets,
    }


def init_stats_tables(conn: sqlite3.Connection) -> None:
    """Create all stats tables and indexes."""
    init_traffic_stats_table(conn)
    init_protocol_stats_table(conn)
    init_address_count_stats_table(conn)
    init_address_structure_stats_table(conn)


def init_traffic_stats_table(conn: sqlite3.Connection) -> None:
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS traffic_stats (
            source_id TEXT NOT NULL,
            granularity TEXT NOT NULL CHECK (granularity IN ('5m', '30m', '1h', '1d')),
            bucket_start INTEGER NOT NULL,
            bucket_end INTEGER NOT NULL,
            ip_version INTEGER NOT NULL CHECK (ip_version IN (4, 6)),
            src_visibility TEXT NOT NULL CHECK (src_visibility IN ('all', 'literal', 'anonymized')),
            dst_visibility TEXT NOT NULL CHECK (dst_visibility IN ('all', 'literal', 'anonymized')),
            flows INTEGER NOT NULL,
            flows_tcp INTEGER NOT NULL,
            flows_udp INTEGER NOT NULL,
            flows_icmp INTEGER NOT NULL,
            flows_other INTEGER NOT NULL,
            packets INTEGER NOT NULL,
            packets_tcp INTEGER NOT NULL,
            packets_udp INTEGER NOT NULL,
            packets_icmp INTEGER NOT NULL,
            packets_other INTEGER NOT NULL,
            bytes INTEGER NOT NULL,
            bytes_tcp INTEGER NOT NULL,
            bytes_udp INTEGER NOT NULL,
            bytes_icmp INTEGER NOT NULL,
            bytes_other INTEGER NOT NULL,
            processed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (
                source_id, granularity, bucket_start, ip_version,
                src_visibility, dst_visibility
            )
        ) WITHOUT ROWID
        """
    )
    conn.execute(
        """
        CREATE INDEX IF NOT EXISTS idx_traffic_stats_query
        ON traffic_stats (
            granularity, bucket_start, source_id, ip_version,
            src_visibility, dst_visibility
        )
        """
    )


def init_protocol_stats_table(conn: sqlite3.Connection) -> None:
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS protocol_stats (
            source_id TEXT NOT NULL,
            granularity TEXT NOT NULL CHECK (granularity IN ('5m', '30m', '1h', '1d')),
            bucket_start INTEGER NOT NULL,
            bucket_end INTEGER NOT NULL,
            ip_version INTEGER NOT NULL CHECK (ip_version IN (4, 6)),
            src_visibility TEXT NOT NULL CHECK (src_visibility IN ('all', 'literal', 'anonymized')),
            dst_visibility TEXT NOT NULL CHECK (dst_visibility IN ('all', 'literal', 'anonymized')),
            unique_protocols_count INTEGER NOT NULL,
            protocols_list TEXT NOT NULL,
            processed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (
                source_id, granularity, bucket_start, ip_version,
                src_visibility, dst_visibility
            )
        ) WITHOUT ROWID
        """
    )
    conn.execute(
        """
        CREATE INDEX IF NOT EXISTS idx_protocol_stats_query
        ON protocol_stats (
            granularity, bucket_start, source_id, ip_version,
            src_visibility, dst_visibility
        )
        """
    )


def init_address_count_stats_table(conn: sqlite3.Connection) -> None:
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS address_count_stats (
            source_id TEXT NOT NULL,
            granularity TEXT NOT NULL CHECK (granularity IN ('5m', '30m', '1h', '1d')),
            bucket_start INTEGER NOT NULL,
            bucket_end INTEGER NOT NULL,
            ip_version INTEGER NOT NULL CHECK (ip_version IN (4, 6)),
            src_visibility TEXT NOT NULL CHECK (src_visibility IN ('all', 'literal', 'anonymized')),
            dst_visibility TEXT NOT NULL CHECK (dst_visibility IN ('all', 'literal', 'anonymized')),
            address_side TEXT NOT NULL CHECK (address_side IN ('source', 'destination')),
            unique_address_count INTEGER NOT NULL,
            processed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (
                source_id, granularity, bucket_start, ip_version,
                src_visibility, dst_visibility, address_side
            )
        ) WITHOUT ROWID
        """
    )
    conn.execute(
        """
        CREATE INDEX IF NOT EXISTS idx_address_count_stats_query
        ON address_count_stats (
            granularity, bucket_start, source_id, ip_version,
            src_visibility, dst_visibility, address_side
        )
        """
    )


def init_address_structure_stats_table(conn: sqlite3.Connection) -> None:
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS address_structure_stats (
            source_id TEXT NOT NULL,
            granularity TEXT NOT NULL CHECK (granularity IN ('5m', '30m', '1h', '1d')),
            bucket_start INTEGER NOT NULL,
            bucket_end INTEGER NOT NULL,
            ip_version INTEGER NOT NULL CHECK (ip_version IN (4, 6)),
            src_visibility TEXT NOT NULL CHECK (src_visibility IN ('all', 'literal', 'anonymized')),
            dst_visibility TEXT NOT NULL CHECK (dst_visibility IN ('all', 'literal', 'anonymized')),
            address_side TEXT NOT NULL CHECK (address_side IN ('source', 'destination')),
            structure_kind TEXT NOT NULL CHECK (structure_kind IN ('structure', 'spectrum', 'dimension')),
            values_json TEXT NOT NULL,
            metadata_json TEXT NOT NULL,
            processed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (
                source_id, granularity, bucket_start, ip_version,
                src_visibility, dst_visibility, address_side, structure_kind
            )
        ) WITHOUT ROWID
        """
    )
    conn.execute(
        """
        CREATE INDEX IF NOT EXISTS idx_address_structure_stats_query
        ON address_structure_stats (
            granularity, bucket_start, source_id, ip_version,
            src_visibility, dst_visibility, address_side, structure_kind
        )
        """
    )


def build_address_structure_stats_rows(
    *,
    source_id: str,
    granularity: str,
    bucket_start: int,
    bucket_end: int,
    ip_version: int,
    src_visibility: str,
    dst_visibility: str,
    address_side: str,
    result: MaadJsonResult,
) -> list[dict]:
    """Build structure/spectrum/dimension rows for one address set."""
    base = {
        'source_id': source_id,
        'granularity': granularity,
        'bucket_start': bucket_start,
        'bucket_end': bucket_end,
        'ip_version': ip_version,
        'src_visibility': src_visibility,
        'dst_visibility': dst_visibility,
        'address_side': address_side,
        'metadata_json': json.dumps(result.metadata, sort_keys=True),
    }
    return [
        {
            **base,
            'structure_kind': 'structure',
            'values_json': json.dumps(result.structure, sort_keys=True),
        },
        {
            **base,
            'structure_kind': 'spectrum',
            'values_json': json.dumps(result.spectrum, sort_keys=True),
        },
        {
            **base,
            'structure_kind': 'dimension',
            'values_json': json.dumps(result.dimensions, sort_keys=True),
        },
    ]


def insert_traffic_stats_rows(conn: sqlite3.Connection, rows: list[dict]) -> None:
    conn.executemany(
        """
        INSERT OR REPLACE INTO traffic_stats (
            source_id, granularity, bucket_start, bucket_end, ip_version,
            src_visibility, dst_visibility,
            flows, flows_tcp, flows_udp, flows_icmp, flows_other,
            packets, packets_tcp, packets_udp, packets_icmp, packets_other,
            bytes, bytes_tcp, bytes_udp, bytes_icmp, bytes_other
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        [
            (
                row['source_id'],
                row['granularity'],
                row['bucket_start'],
                row['bucket_end'],
                row['ip_version'],
                row['src_visibility'],
                row['dst_visibility'],
                row['flows'],
                row['flows_tcp'],
                row['flows_udp'],
                row['flows_icmp'],
                row['flows_other'],
                row['packets'],
                row['packets_tcp'],
                row['packets_udp'],
                row['packets_icmp'],
                row['packets_other'],
                row['bytes'],
                row['bytes_tcp'],
                row['bytes_udp'],
                row['bytes_icmp'],
                row['bytes_other'],
            )
            for row in rows
        ],
    )


def insert_protocol_stats_rows(conn: sqlite3.Connection, rows: list[dict]) -> None:
    conn.executemany(
        """
        INSERT OR REPLACE INTO protocol_stats (
            source_id, granularity, bucket_start, bucket_end, ip_version,
            src_visibility, dst_visibility, unique_protocols_count, protocols_list
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        [
            (
                row['source_id'],
                row['granularity'],
                row['bucket_start'],
                row['bucket_end'],
                row['ip_version'],
                row['src_visibility'],
                row['dst_visibility'],
                row['unique_protocols_count'],
                row['protocols_list'],
            )
            for row in rows
        ],
    )


def insert_address_count_stats_rows(conn: sqlite3.Connection, rows: list[dict]) -> None:
    conn.executemany(
        """
        INSERT OR REPLACE INTO address_count_stats (
            source_id, granularity, bucket_start, bucket_end, ip_version,
            src_visibility, dst_visibility, address_side, unique_address_count
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        [
            (
                row['source_id'],
                row['granularity'],
                row['bucket_start'],
                row['bucket_end'],
                row['ip_version'],
                row['src_visibility'],
                row['dst_visibility'],
                row['address_side'],
                row['unique_address_count'],
            )
            for row in rows
        ],
    )


def insert_address_structure_stats_rows(conn: sqlite3.Connection, rows: list[dict]) -> None:
    conn.executemany(
        """
        INSERT OR REPLACE INTO address_structure_stats (
            source_id, granularity, bucket_start, bucket_end, ip_version,
            src_visibility, dst_visibility, address_side, structure_kind,
            values_json, metadata_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        [
            (
                row['source_id'],
                row['granularity'],
                row['bucket_start'],
                row['bucket_end'],
                row['ip_version'],
                row['src_visibility'],
                row['dst_visibility'],
                row['address_side'],
                row['structure_kind'],
                row['values_json'],
                row['metadata_json'],
            )
            for row in rows
        ],
    )
