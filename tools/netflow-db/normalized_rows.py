"""
Normalized row utilities for pipeline.

Adapters map raw input rows into a shared, bucketed row contract that powers
the aggregate tables.
"""

from __future__ import annotations

import ipaddress
from dataclasses import dataclass
from functools import lru_cache
from typing import Any, Mapping, Sequence

from csv_ingest import (
    TIMESTAMP_KEYS,
    CsvSourceConfig,
    CsvSourceConfigError,
    parse_timestamp,
    resolve_bucket_start,
    resolve_source_id,
)


NFDUMP_CSV_FORMAT = 'csv:%trr,%ter,%tsr,%sa,%da,%sp,%dp,%pr,%pkt,%byt,%stos,%dtos'


@dataclass(slots=True)
class NormalizedRow:
    source_id: str
    bucket_start: int
    bucket_end: int
    time_received: int | None
    time_end: int | None
    time_start: int | None
    src_ip: str
    dst_ip: str
    ip_version: int
    src_port: int
    dst_port: int
    protocol: int
    packets: int
    bytes: int
    src_tos: int
    dst_tos: int


def build_nfdump_csv_command(file_path: str, ip_version: int) -> list[str]:
    """Build the fixed-order nfdump CSV command for a capture file."""
    if ip_version == 4:
        family_filter = ['ipv4']
    elif ip_version == 6:
        family_filter = ['ipv6', '-6']
    else:
        raise ValueError('ip_version must be 4 or 6')

    return ['nfdump', '-r', file_path, '-q', '-o', NFDUMP_CSV_FORMAT, *family_filter]


def normalize_nfdump_csv_values(values: Sequence[str], source_id: str) -> NormalizedRow:
    """Normalize a fixed-order nfdump CSV row."""
    if len(values) != 12:
        raise CsvSourceConfigError(f'nfdump CSV row must contain 12 values, got {len(values)}.')

    row = {
        'tr': values[0],
        'te': values[1],
        'ts': values[2],
        'sa': values[3],
        'da': values[4],
        'sp': normalize_nfdump_port(values[5]),
        'dp': normalize_nfdump_port(values[6]),
        'pr': values[7],
        'pkt': values[8],
        'byt': values[9],
        'stos': values[10],
        'dtos': values[11],
    }
    config = CsvSourceConfig(
        delimiter=',',
        has_header=False,
        timestamp_format='unix',
        datetime_format='%Y-%m-%d %H:%M:%S',
        columns={
            'time_received': 'tr',
            'time_end': 'te',
            'time_start': 'ts',
            'src_ip': 'sa',
            'dst_ip': 'da',
            'src_port': 'sp',
            'dst_port': 'dp',
            'protocol': 'pr',
            'packets': 'pkt',
            'bytes': 'byt',
            'src_tos': 'stos',
            'dst_tos': 'dtos',
        },
        source_id_value=source_id,
        source_id_column=None,
        timestamp_timezone='UTC',
        fieldnames=None,
        protocol_map={},
        skip_bad_column_count=False,
        archive_member_contains=None,
        discovery_include_contains=('csv',),
        discovery_include_suffixes=('.tar.gz', '.tgz'),
        discovery_exclude_suffixes=('.aria2', '.txt'),
        input_order='timestamp_ascending',
        out_of_order_lag_buckets=12,
    )
    return normalize_csv_row(row, config)


def normalize_nfdump_port(raw_value: str) -> str:
    """Map nfdump ICMP type/code pseudo-ports to no transport port."""
    raw_text = str(raw_value).strip()
    if '.' in raw_text:
        return '0'
    return raw_text


def normalize_csv_row(row: Mapping[str, Any], config: CsvSourceConfig) -> NormalizedRow:
    """Normalize a mapped CSV row into the shared row contract."""
    source_id = resolve_source_id(row, config)
    timestamps = extract_timestamps(row, config)
    bucket_start = resolve_bucket_start_from_timestamps(timestamps)
    bucket_end = bucket_start + 300

    src_ip = require_value(row, config.columns['src_ip'])
    dst_ip = require_value(row, config.columns['dst_ip'])
    ip_version = infer_ip_version(src_ip, dst_ip)

    return NormalizedRow(
        source_id=source_id,
        bucket_start=bucket_start,
        bucket_end=bucket_end,
        time_received=timestamps.get('time_received'),
        time_end=timestamps.get('time_end'),
        time_start=timestamps.get('time_start'),
        src_ip=src_ip,
        dst_ip=dst_ip,
        ip_version=ip_version,
        src_port=extract_int(row, config, 'src_port'),
        dst_port=extract_int(row, config, 'dst_port'),
        protocol=extract_protocol(row, config, 'protocol'),
        packets=extract_int(row, config, 'packets'),
        bytes=extract_int(row, config, 'bytes'),
        src_tos=extract_int(row, config, 'src_tos'),
        dst_tos=extract_int(row, config, 'dst_tos'),
    )


def normalize_csv_values(
    values: Sequence[str],
    config: CsvSourceConfig,
    field_indexes: Mapping[str, int],
) -> NormalizedRow:
    """Normalize a headerless CSV row using precomputed field indexes."""
    source_id = resolve_source_id_from_values(values, config, field_indexes)
    timestamps = extract_timestamps_from_values(values, config, field_indexes)
    bucket_start = resolve_bucket_start_from_timestamps(timestamps)
    bucket_end = bucket_start + 300

    src_ip = require_value_from_values(values, field_indexes[config.columns['src_ip']], config.columns['src_ip'])
    dst_ip = require_value_from_values(values, field_indexes[config.columns['dst_ip']], config.columns['dst_ip'])
    ip_version = infer_ip_version(src_ip, dst_ip)

    return NormalizedRow(
        source_id=source_id,
        bucket_start=bucket_start,
        bucket_end=bucket_end,
        time_received=timestamps.get('time_received'),
        time_end=timestamps.get('time_end'),
        time_start=timestamps.get('time_start'),
        src_ip=src_ip,
        dst_ip=dst_ip,
        ip_version=ip_version,
        src_port=extract_int_from_values(values, config, field_indexes, 'src_port'),
        dst_port=extract_int_from_values(values, config, field_indexes, 'dst_port'),
        protocol=extract_protocol_from_values(values, config, field_indexes, 'protocol'),
        packets=extract_int_from_values(values, config, field_indexes, 'packets'),
        bytes=extract_int_from_values(values, config, field_indexes, 'bytes'),
        src_tos=extract_int_from_values(values, config, field_indexes, 'src_tos'),
        dst_tos=extract_int_from_values(values, config, field_indexes, 'dst_tos'),
    )


def resolve_source_id_from_values(
    values: Sequence[str],
    config: CsvSourceConfig,
    field_indexes: Mapping[str, int],
) -> str:
    """Resolve source_id from a constant or precomputed value index."""
    if config.source_id_value is not None:
        return config.source_id_value
    assert config.source_id_column is not None
    return require_value_from_values(
        values,
        field_indexes[config.source_id_column],
        config.source_id_column,
    )


def extract_timestamps(row: Mapping[str, Any], config: CsvSourceConfig) -> dict[str, int]:
    """Extract configured timestamps once for bucket and row fields."""
    timestamps = {}
    for logical_key in TIMESTAMP_KEYS:
        timestamp = extract_timestamp(row, config, logical_key)
        if timestamp is not None:
            timestamps[logical_key] = timestamp
    return timestamps


def extract_timestamps_from_values(
    values: Sequence[str],
    config: CsvSourceConfig,
    field_indexes: Mapping[str, int],
) -> dict[str, int]:
    """Extract configured timestamps from indexed values."""
    timestamps = {}
    for logical_key in TIMESTAMP_KEYS:
        column_name = config.columns.get(logical_key)
        if column_name is None:
            continue
        raw = values[field_indexes[column_name]]
        if raw in (None, ''):
            continue
        timestamps[logical_key] = parse_timestamp(
            raw,
            config.timestamp_format,
            config.timestamp_timezone,
            config.datetime_format,
        )
    return timestamps


def resolve_bucket_start_from_timestamps(timestamps: Mapping[str, int]) -> int:
    """Resolve a bucket from already-parsed timestamps."""
    for logical_key in TIMESTAMP_KEYS:
        timestamp = timestamps.get(logical_key)
        if timestamp is not None:
            return timestamp - (timestamp % 300)
    raise CsvSourceConfigError(
        'CSV row did not contain any usable timestamp value for the configured precedence.'
    )


def infer_ip_version(src_ip: str, dst_ip: str) -> int:
    """Infer a shared IP family from the parsed source and destination addresses."""
    try:
        src_version = parse_ip_version(src_ip)
        dst_version = parse_ip_version(dst_ip)
    except ValueError as error:
        raise CsvSourceConfigError(
            f'Invalid IP address value in row: {src_ip} -> {dst_ip}.'
        ) from error
    if src_version != dst_version:
        raise CsvSourceConfigError(
            f'Mixed IP versions in one row are not supported: {src_ip} -> {dst_ip}.'
        )
    return src_version


@lru_cache(maxsize=1_000_000)
def parse_ip_version(raw_value: str) -> int:
    """Parse and cache IP versions for heavily repeated endpoint values."""
    if '.' in raw_value and ':' not in raw_value:
        parse_ipv4_address(raw_value)
        return 4
    return ipaddress.ip_address(raw_value).version


def parse_ipv4_address(raw_value: str) -> None:
    """Validate a dotted-quad IPv4 address without the ipaddress module overhead."""
    parts = raw_value.split('.')
    if len(parts) != 4:
        raise ValueError(f'Invalid IPv4 address: {raw_value}')
    for part in parts:
        if part == '' or not part.isdigit():
            raise ValueError(f'Invalid IPv4 address: {raw_value}')
        value = int(part)
        if value < 0 or value > 255:
            raise ValueError(f'Invalid IPv4 address: {raw_value}')


def require_value(row: Mapping[str, Any], column_name: str) -> str:
    """Load a required string value from the row."""
    raw = row.get(column_name)
    if raw is None:
        raise CsvSourceConfigError(f"CSV row is missing required value for column '{column_name}'.")
    value = str(raw).strip()
    if value == '':
        raise CsvSourceConfigError(f"CSV row is missing required value for column '{column_name}'.")
    return value


def require_value_from_values(values: Sequence[str], index: int, column_name: str) -> str:
    """Load a required string value from a pre-indexed row."""
    value = values[index].strip()
    if value == '':
        raise CsvSourceConfigError(f"CSV row is missing required value for column '{column_name}'.")
    return value


def extract_timestamp(row: Mapping[str, Any], config: CsvSourceConfig, logical_key: str) -> int | None:
    """Extract an optional timestamp field using the config mapping."""
    column_name = config.columns.get(logical_key)
    if column_name is None:
        return None
    raw = row.get(column_name)
    if raw in (None, ''):
        return None
    return parse_timestamp(
        raw,
        config.timestamp_format,
        config.timestamp_timezone,
        config.datetime_format,
    )


def extract_int(row: Mapping[str, Any], config: CsvSourceConfig, logical_key: str) -> int:
    """Extract an optional integer field, defaulting to 0 when absent."""
    column_name = config.columns.get(logical_key)
    if column_name is None:
        return 0
    raw = row.get(column_name)
    if raw in (None, ''):
        return 0
    try:
        return int(str(raw).strip())
    except ValueError as error:
        raise CsvSourceConfigError(
            f"Invalid integer value '{raw}' for column '{column_name}'."
        ) from error


def extract_int_from_values(
    values: Sequence[str],
    config: CsvSourceConfig,
    field_indexes: Mapping[str, int],
    logical_key: str,
) -> int:
    """Extract an integer field from pre-indexed values."""
    column_name = config.columns.get(logical_key)
    if column_name is None:
        return 0
    raw = values[field_indexes[column_name]]
    if raw == '':
        return 0
    try:
        return int(raw)
    except ValueError as error:
        raise CsvSourceConfigError(
            f"Invalid integer value '{raw}' for column '{column_name}'."
        ) from error


def extract_protocol(row: Mapping[str, Any], config: CsvSourceConfig, logical_key: str) -> int:
    """Extract protocol as either IANA number or common NetFlow protocol name."""
    column_name = config.columns.get(logical_key)
    if column_name is None:
        return 0
    raw = row.get(column_name)
    if raw in (None, ''):
        return 0
    raw_text = str(raw).strip()
    protocol = config.protocol_map.get(raw_text.upper())
    if protocol is not None:
        return protocol
    try:
        return int(raw_text)
    except ValueError as error:
        raise CsvSourceConfigError(
            f"Invalid protocol value '{raw}' for column '{column_name}'."
        ) from error


def extract_protocol_from_values(
    values: Sequence[str],
    config: CsvSourceConfig,
    field_indexes: Mapping[str, int],
    logical_key: str,
) -> int:
    """Extract protocol from pre-indexed values."""
    column_name = config.columns.get(logical_key)
    if column_name is None:
        return 0
    raw_text = values[field_indexes[column_name]].strip()
    if raw_text == '':
        return 0
    protocol = config.protocol_map.get(raw_text.upper())
    if protocol is not None:
        return protocol
    try:
        return int(raw_text)
    except ValueError as error:
        raise CsvSourceConfigError(
            f"Invalid protocol value '{raw_text}' for column '{column_name}'."
        ) from error
