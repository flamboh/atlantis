"""
CSV ingest contract for pipeline.

Strictly validates user-provided CSV mapping json and resolves the bucket/source
dimensions needed by downstream aggregators.
"""

from __future__ import annotations

import json
import math
from dataclasses import dataclass
from datetime import datetime
from functools import lru_cache
from pathlib import Path
from typing import Any, Mapping
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError


TIMESTAMP_KEYS = ('time_received', 'time_end', 'time_start')
REQUIRED_COLUMN_KEYS = ('src_ip', 'dst_ip')


class CsvSourceConfigError(ValueError):
    """Raised when a CSV source mapping is invalid or incomplete."""


@dataclass(frozen=True)
class CsvSourceConfig:
    delimiter: str
    has_header: bool
    timestamp_format: str
    datetime_format: str
    timestamp_timezone: str
    fieldnames: list[str] | None
    columns: dict[str, str]
    protocol_map: dict[str, int]
    source_id_value: str | None
    source_id_column: str | None
    skip_bad_column_count: bool
    archive_member_contains: str | None
    discovery_include_contains: tuple[str, ...]
    discovery_include_suffixes: tuple[str, ...]
    discovery_exclude_suffixes: tuple[str, ...]
    input_order: str
    out_of_order_lag_buckets: int


def load_csv_source_config(path: str | Path) -> CsvSourceConfig:
    """Load and validate a CSV source mapping file."""
    config_path = Path(path)
    with open(config_path, 'r', encoding='utf-8') as handle:
        payload = json.load(handle)
    return validate_csv_source_config(payload)


def validate_csv_source_config(payload: Mapping[str, Any]) -> CsvSourceConfig:
    """Validate a raw mapping payload and return the normalized config."""
    if not isinstance(payload, Mapping):
        raise CsvSourceConfigError('CSV config root must be a JSON object.')

    columns_raw = payload.get('columns')
    if not isinstance(columns_raw, Mapping):
        raise CsvSourceConfigError("CSV config must include a 'columns' object.")

    columns: dict[str, str] = {}
    for key, value in columns_raw.items():
        if value in (None, ''):
            continue
        if not isinstance(key, str) or not isinstance(value, str):
            raise CsvSourceConfigError('CSV column mappings must be string-to-string entries.')
        columns[key] = value

    if not any(key in columns for key in TIMESTAMP_KEYS):
        raise CsvSourceConfigError(
            "CSV config must map at least one timestamp column: "
            "time_received, time_end, or time_start."
        )

    for key in REQUIRED_COLUMN_KEYS:
        if key not in columns:
            raise CsvSourceConfigError(f"CSV config is missing required column mapping '{key}'.")

    source_id = payload.get('source_id')
    source_id_value: str | None = None
    source_id_column: str | None = None
    if isinstance(source_id, Mapping):
        raw_value = source_id.get('value')
        raw_column = source_id.get('column')
        if raw_value not in (None, ''):
            if not isinstance(raw_value, str):
                raise CsvSourceConfigError("CSV config source_id.value must be a string.")
            source_id_value = raw_value
        if raw_column not in (None, ''):
            if not isinstance(raw_column, str):
                raise CsvSourceConfigError("CSV config source_id.column must be a string.")
            source_id_column = raw_column

    if source_id_value is None and source_id_column is None:
        raise CsvSourceConfigError(
            "CSV config must declare source_id via either source_id.value or source_id.column."
        )

    delimiter = payload.get('delimiter', ',')
    if not isinstance(delimiter, str) or len(delimiter) != 1:
        raise CsvSourceConfigError("CSV config delimiter must be a single character string.")

    has_header = payload.get('has_header', True)
    if not isinstance(has_header, bool):
        raise CsvSourceConfigError('CSV config has_header must be true or false.')

    fieldnames = parse_optional_string_list(payload.get('fieldnames'), 'fieldnames')
    if not has_header and fieldnames is None:
        raise CsvSourceConfigError('CSV config fieldnames are required when has_header is false.')

    skip_bad_column_count = payload.get('skip_bad_column_count', False)
    if not isinstance(skip_bad_column_count, bool):
        raise CsvSourceConfigError('CSV config skip_bad_column_count must be true or false.')

    protocol_map = parse_protocol_map(payload.get('protocol_map'))

    archive = payload.get('archive', {})
    if archive is None:
        archive = {}
    if not isinstance(archive, Mapping):
        raise CsvSourceConfigError('CSV config archive must be an object.')
    archive_member_contains = archive.get('member_contains')
    if archive_member_contains is not None and not isinstance(archive_member_contains, str):
        raise CsvSourceConfigError('CSV config archive.member_contains must be a string.')

    discovery = payload.get('discovery', {})
    if discovery is None:
        discovery = {}
    if not isinstance(discovery, Mapping):
        raise CsvSourceConfigError('CSV config discovery must be an object.')
    discovery_include_contains_raw = parse_optional_string_list(
        discovery.get('include_contains'), 'discovery.include_contains'
    )
    discovery_include_suffixes_raw = parse_optional_string_list(
        discovery.get('include_suffixes'), 'discovery.include_suffixes'
    )
    discovery_exclude_suffixes_raw = parse_optional_string_list(
        discovery.get('exclude_suffixes'), 'discovery.exclude_suffixes'
    )
    discovery_include_contains = tuple(
        ['csv'] if discovery_include_contains_raw is None else discovery_include_contains_raw
    )
    discovery_include_suffixes = tuple(
        ['.tar.gz', '.tgz']
        if discovery_include_suffixes_raw is None
        else discovery_include_suffixes_raw
    )
    discovery_exclude_suffixes = tuple(
        ['.aria2', '.txt']
        if discovery_exclude_suffixes_raw is None
        else discovery_exclude_suffixes_raw
    )

    timestamp_format = payload.get('timestamp_format', 'unix')
    if timestamp_format not in {'unix', 'unix_ms', 'datetime'}:
        raise CsvSourceConfigError(
            "CSV config timestamp_format must be 'unix', 'unix_ms', or 'datetime'."
        )

    datetime_format = payload.get('datetime_format', '%Y-%m-%d %H:%M:%S')
    if not isinstance(datetime_format, str) or datetime_format == '':
        raise CsvSourceConfigError('CSV config datetime_format must be a non-empty string.')

    timestamp_timezone = payload.get('timestamp_timezone', 'UTC')
    if not isinstance(timestamp_timezone, str):
        raise CsvSourceConfigError('CSV config timestamp_timezone must be a string.')
    try:
        ZoneInfo(timestamp_timezone)
    except ZoneInfoNotFoundError as error:
        raise CsvSourceConfigError(
            f"CSV config timestamp_timezone is not a valid IANA timezone: {timestamp_timezone}."
        ) from error

    input_order = payload.get('input_order', 'timestamp_ascending')
    if input_order not in {'timestamp_ascending', 'unsorted'}:
        raise CsvSourceConfigError(
            "CSV config input_order must be 'timestamp_ascending' or 'unsorted'."
        )

    out_of_order_lag_buckets = payload.get('out_of_order_lag_buckets', 12)
    if (
        not isinstance(out_of_order_lag_buckets, int)
        or isinstance(out_of_order_lag_buckets, bool)
        or out_of_order_lag_buckets < 0
    ):
        raise CsvSourceConfigError('CSV config out_of_order_lag_buckets must be an integer >= 0.')

    return CsvSourceConfig(
        delimiter=delimiter,
        has_header=has_header,
        timestamp_format=timestamp_format,
        datetime_format=datetime_format,
        timestamp_timezone=timestamp_timezone,
        fieldnames=fieldnames,
        columns=columns,
        protocol_map=protocol_map,
        source_id_value=source_id_value,
        source_id_column=source_id_column,
        skip_bad_column_count=skip_bad_column_count,
        archive_member_contains=archive_member_contains,
        discovery_include_contains=discovery_include_contains,
        discovery_include_suffixes=discovery_include_suffixes,
        discovery_exclude_suffixes=discovery_exclude_suffixes,
        input_order=input_order,
        out_of_order_lag_buckets=out_of_order_lag_buckets,
    )


def parse_optional_string_list(raw_value: Any, name: str) -> list[str] | None:
    """Parse an optional JSON string list field."""
    if raw_value is None:
        return None
    if (
        not isinstance(raw_value, list)
        or not all(isinstance(value, str) and value != '' for value in raw_value)
    ):
        raise CsvSourceConfigError(f'CSV config {name} must be a list of strings.')
    return raw_value


def parse_protocol_map(raw_value: Any) -> dict[str, int]:
    """Parse optional protocol-name to IANA-number mappings."""
    protocol_map = default_protocol_map()
    if raw_value is None:
        return protocol_map
    if not isinstance(raw_value, Mapping):
        raise CsvSourceConfigError('CSV config protocol_map must be an object.')
    for name, number in raw_value.items():
        if not isinstance(name, str) or not isinstance(number, int):
            raise CsvSourceConfigError('CSV config protocol_map must map strings to integers.')
        protocol_map[name.upper()] = number
    return protocol_map


def default_protocol_map() -> dict[str, int]:
    """Load protocol aliases from /etc/protocols, with a small static fallback."""
    fallback = {
        'ICMP': 1,
        'IPIP': 4,
        'TCP': 6,
        'EGP': 8,
        'UDP': 17,
        'RSVP': 46,
        'GRE': 47,
        'ESP': 50,
        'AH': 51,
        'ICMPV6': 58,
        'IPV6-ICMP': 58,
        'EIGRP': 88,
        'OSPF': 89,
        'OSPFIGP': 89,
        'PIM': 103,
        'SCTP': 132,
    }
    protocols_path = Path('/etc/protocols')
    if not protocols_path.is_file():
        return fallback

    protocol_map = dict(fallback)
    for line in protocols_path.read_text(encoding='utf-8').splitlines():
        fields = line.split('#', 1)[0].split()
        if len(fields) < 2:
            continue
        try:
            number = int(fields[1])
        except ValueError:
            continue
        for name in fields[:1] + fields[2:]:
            protocol_map[name.upper()] = number
    return protocol_map


def resolve_source_id(row: Mapping[str, Any], config: CsvSourceConfig) -> str:
    """Resolve the row source_id from a constant or mapped source column."""
    if config.source_id_value is not None:
        return config.source_id_value
    assert config.source_id_column is not None
    raw_value = row.get(config.source_id_column)
    if raw_value is None:
        raise CsvSourceConfigError(
            f"CSV row is missing source_id column '{config.source_id_column}'."
        )
    source_id = str(raw_value).strip()
    if source_id == '':
        raise CsvSourceConfigError(
            f"CSV row is missing source_id column '{config.source_id_column}'."
        )
    return source_id


def resolve_bucket_start(
    row: Mapping[str, Any],
    config: CsvSourceConfig,
    bucket_seconds: int = 300,
) -> int:
    """Resolve a row timestamp using configured precedence and floor to a bucket."""
    for logical_key in TIMESTAMP_KEYS:
        column_name = config.columns.get(logical_key)
        if column_name is None:
            continue
        raw_value = row.get(column_name)
        if raw_value in (None, ''):
            continue
        unix_ts = parse_timestamp(
            raw_value,
            config.timestamp_format,
            config.timestamp_timezone,
            config.datetime_format,
        )
        return floor_unix_timestamp(unix_ts, bucket_seconds)

    raise CsvSourceConfigError(
        'CSV row did not contain any usable timestamp value for the configured precedence.'
    )


def parse_timestamp(
    raw_value: Any,
    timestamp_format: str,
    timestamp_timezone: str = 'UTC',
    datetime_format: str = '%Y-%m-%d %H:%M:%S',
) -> int:
    """Parse the configured timestamp value into unix seconds."""
    raw_text = str(raw_value).strip()

    if timestamp_format == 'datetime':
        return parse_datetime_timestamp(raw_text, timestamp_timezone, datetime_format)
    return parse_numeric_timestamp(raw_text, timestamp_format)


@lru_cache(maxsize=1_000_000)
def parse_datetime_timestamp(raw_text: str, timestamp_timezone: str, datetime_format: str) -> int:
    """Parse and cache datetime timestamps that repeat heavily in flow rows."""
    try:
        parsed = datetime.strptime(raw_text, datetime_format)
    except ValueError as error:
        raise CsvSourceConfigError(f"Invalid timestamp value '{raw_text}'.") from error
    return int(parsed.replace(tzinfo=ZoneInfo(timestamp_timezone)).timestamp())


@lru_cache(maxsize=1_000_000)
def parse_numeric_timestamp(raw_text: str, timestamp_format: str) -> int:
    """Parse and cache numeric timestamp values."""
    try:
        numeric = float(raw_text)
    except ValueError as error:
        raise CsvSourceConfigError(f"Invalid timestamp value '{raw_text}'.") from error

    if not math.isfinite(numeric):
        raise CsvSourceConfigError(f"Invalid timestamp value '{raw_text}'.")
    if numeric < -(1 << 63) or numeric > (1 << 63) - 1:
        raise CsvSourceConfigError(f"Invalid timestamp value '{raw_text}'.")
    try:
        if timestamp_format == 'unix':
            return int(numeric)
        if timestamp_format == 'unix_ms':
            return int(numeric) // 1000
    except (OverflowError, ValueError) as error:
        raise CsvSourceConfigError(f"Invalid timestamp value '{raw_text}'.") from error
    raise CsvSourceConfigError(f"Unsupported timestamp_format '{timestamp_format}'.")


def floor_unix_timestamp(unix_ts: int, bucket_seconds: int) -> int:
    """Floor a unix timestamp to the start of its bucket."""
    if bucket_seconds <= 0:
        raise CsvSourceConfigError('bucket_seconds must be > 0.')
    return unix_ts - (unix_ts % bucket_seconds)
