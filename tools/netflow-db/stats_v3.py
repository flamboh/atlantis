"""Dimensioned v3 aggregate tables with address visibility scope."""

from __future__ import annotations

import json
import sqlite3
from typing import Iterable

from maad_v2 import MaadJsonResult


GRANULARITIES = ('5m', '30m', '1h', '1d')
VISIBILITIES = ('all', 'literal', 'anonymized')
ADDRESS_SIDES = ('source', 'destination')
STRUCTURE_KINDS = ('structure', 'spectrum', 'dimension')
ALL_VISIBILITY = ('all', 'all')
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


def protocol_metric_keys(protocol: int | str) -> tuple[str, str, str]:
    protocol_value = int(protocol)
    if protocol_value == 6:
        suffix = 'tcp'
    elif protocol_value == 17:
        suffix = 'udp'
    elif protocol_value in (1, 58):
        suffix = 'icmp'
    else:
        suffix = 'other'
    return f'flows_{suffix}', f'packets_{suffix}', f'bytes_{suffix}'


def validate_ip_version(ip_version: int) -> int:
    if ip_version not in (4, 6):
        raise ValueError(f'Unsupported ip_version: {ip_version!r}')
    return ip_version


def init_stats_v3_tables(conn: sqlite3.Connection) -> None:
    """Create all v3 stats tables and indexes."""
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


def visibility_pair_from_tos(src_tos: int) -> tuple[str, str]:
    """Decode UO anonymization low bits from source ToS."""
    src_visibility = 'anonymized' if src_tos & 2 else 'literal'
    dst_visibility = 'anonymized' if src_tos & 1 else 'literal'
    return src_visibility, dst_visibility


def visibility_pairs_for_row(src_tos: int) -> tuple[tuple[str, str], tuple[str, str]]:
    """Return all-traffic and exact visibility pairs for one flow row."""
    return ALL_VISIBILITY, visibility_pair_from_tos(src_tos)


def empty_traffic_stats_row(
    *,
    source_id: str,
    granularity: str,
    bucket_start: int,
    bucket_end: int,
    ip_version: int,
    src_visibility: str,
    dst_visibility: str,
) -> dict:
    """Create an empty v3 traffic metric row."""
    validate_ip_version(ip_version)
    return {
        'source_id': source_id,
        'granularity': granularity,
        'bucket_start': bucket_start,
        'bucket_end': bucket_end,
        'ip_version': ip_version,
        'src_visibility': src_visibility,
        'dst_visibility': dst_visibility,
        **{column: 0 for column in NETFLOW_METRIC_COLUMNS},
    }


def add_traffic_metrics_v3(
    row: dict,
    *,
    protocol: int,
    flows: int,
    packets: int,
    bytes_count: int,
) -> None:
    """Add already-grouped traffic metrics to a v3 row."""
    row['flows'] += flows
    row['packets'] += packets
    row['bytes'] += bytes_count
    flow_key, packets_key, bytes_key = protocol_metric_keys(protocol)
    row[flow_key] += flows
    row[packets_key] += packets
    row[bytes_key] += bytes_count


def traffic_key(row: dict) -> tuple:
    return (
        row['source_id'],
        row['granularity'],
        row['bucket_start'],
        row['ip_version'],
        row['src_visibility'],
        row['dst_visibility'],
    )


def protocol_set_entries_to_rows(entries: Iterable[dict]) -> list[dict]:
    """Turn raw protocol-set entries into protocol_stats rows."""
    rows = []
    for entry in entries:
        protocols = sorted(str(protocol) for protocol in entry['protocols'])
        rows.append(
            {
                'source_id': entry['source_id'],
                'granularity': entry['granularity'],
                'bucket_start': entry['bucket_start'],
                'bucket_end': entry['bucket_end'],
                'ip_version': entry['ip_version'],
                'src_visibility': entry['src_visibility'],
                'dst_visibility': entry['dst_visibility'],
                'unique_protocols_count': len(protocols),
                'protocols_list': ','.join(protocols),
            }
        )
    return sorted(rows, key=traffic_key)


def address_set_entries_to_count_rows(entries: Iterable[dict]) -> list[dict]:
    """Turn raw address-set entries into address_count_stats rows."""
    rows = []
    for entry in entries:
        rows.append(
            {
                'source_id': entry['source_id'],
                'granularity': entry['granularity'],
                'bucket_start': entry['bucket_start'],
                'bucket_end': entry['bucket_end'],
                'ip_version': entry['ip_version'],
                'src_visibility': entry['src_visibility'],
                'dst_visibility': entry['dst_visibility'],
                'address_side': entry['address_side'],
                'unique_address_count': len(entry['addresses']),
            }
        )
    return sorted(
        rows,
        key=lambda row: (
            *traffic_key(row),
            row['address_side'],
        ),
    )


def merge_traffic_rows(rows: Iterable[dict]) -> list[dict]:
    """Merge additive v3 traffic rows."""
    merged = {}
    for row in rows:
        key = traffic_key(row)
        target = merged.setdefault(
            key,
            empty_traffic_stats_row(
                source_id=row['source_id'],
                granularity=row['granularity'],
                bucket_start=row['bucket_start'],
                bucket_end=row['bucket_end'],
                ip_version=row['ip_version'],
                src_visibility=row['src_visibility'],
                dst_visibility=row['dst_visibility'],
            ),
        )
        for column in NETFLOW_METRIC_COLUMNS:
            target[column] += row[column]
    return [merged[key] for key in sorted(merged)]


def merge_protocol_set_entries(entries: Iterable[dict]) -> list[dict]:
    """Union protocol sets by v3 dimensions."""
    return protocol_set_entries_to_rows(merge_protocol_set_entries_raw(entries))


def merge_protocol_set_entries_raw(entries: Iterable[dict]) -> list[dict]:
    """Union protocol sets by v3 dimensions and keep raw set entries."""
    merged = {}
    metadata = {}
    for entry in entries:
        key = (
            entry['source_id'],
            entry['granularity'],
            entry['bucket_start'],
            entry['ip_version'],
            entry['src_visibility'],
            entry['dst_visibility'],
        )
        merged.setdefault(key, set()).update(str(protocol) for protocol in entry['protocols'])
        metadata[key] = entry
    return [
        {
            **metadata[key],
            'protocols': sorted(protocols),
        }
        for key, protocols in merged.items()
    ]


def merge_address_set_entries(entries: Iterable[dict]) -> list[dict]:
    """Union address sets by v3 dimensions."""
    merged = {}
    metadata = {}
    for entry in entries:
        key = (
            entry['source_id'],
            entry['granularity'],
            entry['bucket_start'],
            entry['ip_version'],
            entry['src_visibility'],
            entry['dst_visibility'],
            entry['address_side'],
        )
        merged.setdefault(key, set()).update(entry['addresses'])
        metadata[key] = entry
    return [
        {
            **metadata[key],
            'addresses': sorted(addresses),
        }
        for key, addresses in sorted(merged.items())
    ]


def aggregate_raw_v3_entries(
    raw_buckets: list[dict],
    *,
    granularity: str,
    bucket_start: int,
    bucket_end: int,
) -> dict:
    """Build raw v3 aggregate rows/sets from raw 5m buckets."""
    traffic_rows = []
    protocol_entries = []
    address_entries = []

    for raw in raw_buckets:
        for row in raw['traffic_v3_rows']:
            traffic_rows.append(
                {
                    **row,
                    'granularity': granularity,
                    'bucket_start': bucket_start,
                    'bucket_end': bucket_end,
                }
            )
        for entry in raw['protocol_v3_sets']:
            protocol_entries.append(
                {
                    **entry,
                    'granularity': granularity,
                    'bucket_start': bucket_start,
                    'bucket_end': bucket_end,
                }
            )
        for entry in raw['address_v3_sets']:
            address_entries.append(
                {
                    **entry,
                    'granularity': granularity,
                    'bucket_start': bucket_start,
                    'bucket_end': bucket_end,
                }
            )

    merged_protocol_entries = merge_protocol_set_entries_raw(protocol_entries)
    merged_address_entries = merge_address_set_entries(address_entries)
    return {
        'traffic_v3_rows': merge_traffic_rows(traffic_rows),
        'protocol_v3_rows': protocol_set_entries_to_rows(merged_protocol_entries),
        'protocol_v3_sets': merged_protocol_entries,
        'address_count_v3_rows': address_set_entries_to_count_rows(merged_address_entries),
        'address_v3_sets': merged_address_entries,
    }


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
    """Build structure/spectrum/dimension v3 rows for one address set."""
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
