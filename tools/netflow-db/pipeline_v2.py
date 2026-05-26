#!/usr/bin/env python3
"""
Pipeline v2 entrypoint.

Processes explicit csv and nfcapd inputs into:
- processed_inputs_v2
- netflow_stats_v2
- ip_stats_v2
- protocol_stats_v2
- structure_stats_v2
- spectrum_stats_v2
- dimension_stats_v2
"""

from __future__ import annotations

import argparse
import csv
import json
import logging
import os
import sqlite3
import subprocess
import tarfile
from collections import defaultdict
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from functools import lru_cache
from multiprocessing import Pool
from pathlib import Path
from typing import Iterable
from zoneinfo import ZoneInfo

from csv_ingest_v2 import (
    TIMESTAMP_KEYS,
    CsvSourceConfig,
    CsvSourceConfigError,
    load_csv_source_config,
    parse_timestamp,
)
from csv_inputs_v2 import (
    build_field_indexes,
    discover_csv_specs,
    iter_csv_rows,
    iter_headerless_csv_values,
    is_tar_archive,
)
from datasets_metadata import init_datasets_table, upsert_dataset_metadata
from maad_v2 import (
    MaadTimeoutError,
    MaadJsonResult,
    build_maad_v2_rows,
    compute_maad_json,
    init_maad_v2_tables,
    insert_maad_v2_rows,
    run_maad_json,
)
from nfdump_stats_v2 import (
    build_nfcapd_bucket_payload,
    is_nfcapd_bucket_filename,
    parse_nfcapd_bucket_start,
)
from normalized_rows_v2 import NormalizedRow, build_nfdump_csv_command, normalize_nfdump_csv_values
from normalized_rows_v2 import infer_ip_version
from processed_inputs_v2 import (
    init_processed_inputs_v2_table,
    mark_input_bucket_status,
    upsert_input_bucket,
)
from stats_v2 import (
    NETFLOW_METRIC_COLUMNS,
    init_ip_stats_v2_table,
    init_netflow_stats_v2_table,
    init_protocol_stats_v2_table,
    insert_ip_stats_v2_rows,
    insert_netflow_stats_aggregate_v2_rows,
    insert_netflow_stats_v2_rows,
    insert_protocol_stats_v2_rows,
    protocol_metric_keys,
    validate_ip_version,
)


DEFAULT_MAAD_BIN = Path(__file__).resolve().parent / 'maad_fast'
DEFAULT_MAX_WORKERS = int(os.environ.get('MAX_WORKERS', '8'))
DEFAULT_AGGREGATE_MAAD_MAX_WORKERS = int(os.environ.get('AGGREGATE_MAAD_MAX_WORKERS', '4'))
PIPELINE_TIMEZONE = ZoneInfo(os.environ.get('NETFLOW_TIMEZONE', 'America/Los_Angeles'))
FIVE_MINUTE_SECONDS = 300
ARROW_IPV4_REGEX = (
    r'^(?:[0-9]{1,2}|0[0-9]{2}|1[0-9]{2}|2[0-4][0-9]|25[0-5])'
    r'(?:\.(?:[0-9]{1,2}|0[0-9]{2}|1[0-9]{2}|2[0-4][0-9]|25[0-5])){3}$'
)
CSV_STREAM_PROGRESS_ROWS = int(os.environ.get('CSV_STREAM_PROGRESS_ROWS', '1000000'))
CSV_ARROW_BLOCK_BYTES = int(os.environ.get('CSV_ARROW_BLOCK_BYTES', str(64 * 1024 * 1024)))
CSV_MAAD_BATCH_BUCKETS = int(os.environ.get('CSV_MAAD_BATCH_BUCKETS', '8'))
LOGGER = logging.getLogger(__name__)

AGGREGATE_GRANULARITY_SECONDS = (('30m', 1800), ('1h', 3600), ('1d', 86400))
MAAD_TIMEOUT_SECONDS_BY_GRANULARITY = {
    '5m': int(os.environ.get('MAAD_TIMEOUT_5M_SECONDS', '300')),
    '30m': int(os.environ.get('MAAD_TIMEOUT_30M_SECONDS', '600')),
    '1h': int(os.environ.get('MAAD_TIMEOUT_1H_SECONDS', '900')),
    '1d': int(os.environ.get('MAAD_TIMEOUT_1D_SECONDS', '1800')),
}
NFDUMP_HEADER_FIRST_VALUES = {
    'trr',
    'ter',
    'tsr',
    'ts',
    'time_received',
    'time received',
    'received',
}


@dataclass
class BucketAccumulator:
    """Per-source 5-minute accumulator for v2 stats and MAAD inputs."""

    source_id: str
    bucket_start: int
    bucket_end: int
    netflow_by_version: dict[int, dict] = field(default_factory=dict)
    source_ipv4: set[str] = field(default_factory=set)
    destination_ipv4: set[str] = field(default_factory=set)
    source_ipv6: set[str] = field(default_factory=set)
    destination_ipv6: set[str] = field(default_factory=set)
    protocols_ipv4: set[str] = field(default_factory=set)
    protocols_ipv6: set[str] = field(default_factory=set)
    maad_source_ipv4: set[str] = field(default_factory=set)
    maad_destination_ipv4: set[str] = field(default_factory=set)

    def add(self, row: NormalizedRow) -> None:
        """Accumulate one normalized row."""
        self.add_flow(
            ip_version=row.ip_version,
            src_ip=row.src_ip,
            dst_ip=row.dst_ip,
            protocol=row.protocol,
            packets=row.packets,
            bytes_count=row.bytes,
        )

    def add_flow(
        self,
        *,
        ip_version: int,
        src_ip: str,
        dst_ip: str,
        protocol: int,
        packets: int,
        bytes_count: int,
    ) -> None:
        """Accumulate one flow row without constructing a NormalizedRow."""
        self.add_netflow_metrics(
            ip_version=ip_version,
            protocol=protocol,
            flows=1,
            packets=packets,
            bytes_count=bytes_count,
        )

        if ip_version == 4:
            self.source_ipv4.add(src_ip)
            self.destination_ipv4.add(dst_ip)
            self.protocols_ipv4.add(str(protocol))
            self.maad_source_ipv4.add(src_ip)
            self.maad_destination_ipv4.add(dst_ip)
        elif ip_version == 6:
            self.source_ipv6.add(src_ip)
            self.destination_ipv6.add(dst_ip)
            self.protocols_ipv6.add(str(protocol))

    def add_netflow_metrics(
        self,
        *,
        ip_version: int,
        protocol: int,
        flows: int,
        packets: int,
        bytes_count: int,
    ) -> None:
        """Accumulate already-grouped netflow metrics."""
        ip_version = validate_ip_version(ip_version)
        netflow = self.netflow_by_version.setdefault(
            ip_version,
            new_netflow_bucket_from_values(
                source_id=self.source_id,
                bucket_start=self.bucket_start,
                bucket_end=self.bucket_end,
                ip_version=ip_version,
            ),
        )
        netflow['flows'] += flows
        netflow['packets'] += packets
        netflow['bytes'] += bytes_count
        flow_key, packets_key, bytes_key = protocol_metric_keys(protocol)
        netflow[flow_key] += flows
        netflow[packets_key] += packets
        netflow[bytes_key] += bytes_count
        if ip_version == 4:
            self.protocols_ipv4.add(str(protocol))
        elif ip_version == 6:
            self.protocols_ipv6.add(str(protocol))

    def netflow_rows(self) -> list[dict]:
        """Return netflow_stats_v2 rows for this bucket."""
        return [self.netflow_by_version[ip_version] for ip_version in sorted(self.netflow_by_version)]

    def ip_row(self) -> dict:
        """Return one ip_stats_v2 row for this bucket."""
        return {
            'source_id': self.source_id,
            'granularity': '5m',
            'bucket_start': self.bucket_start,
            'bucket_end': self.bucket_end,
            'sa_ipv4_count': len(self.source_ipv4),
            'da_ipv4_count': len(self.destination_ipv4),
            'sa_ipv6_count': len(self.source_ipv6),
            'da_ipv6_count': len(self.destination_ipv6),
        }

    def protocol_row(self) -> dict:
        """Return one protocol_stats_v2 row for this bucket."""
        return {
            'source_id': self.source_id,
            'granularity': '5m',
            'bucket_start': self.bucket_start,
            'bucket_end': self.bucket_end,
            'unique_protocols_count_ipv4': len(self.protocols_ipv4),
            'unique_protocols_count_ipv6': len(self.protocols_ipv6),
            'protocols_list_ipv4': ','.join(sorted(self.protocols_ipv4)),
            'protocols_list_ipv6': ','.join(sorted(self.protocols_ipv6)),
        }

    def raw_bucket_row(self) -> dict:
        """Return raw set payloads needed for cross-bucket aggregate rows."""
        return {
            'source_id': self.source_id,
            'bucket_start': self.bucket_start,
            'source_ipv4': sorted(self.source_ipv4),
            'destination_ipv4': sorted(self.destination_ipv4),
            'source_ipv6': sorted(self.source_ipv6),
            'destination_ipv6': sorted(self.destination_ipv6),
            'protocols_ipv4': sorted(self.protocols_ipv4),
            'protocols_ipv6': sorted(self.protocols_ipv6),
            'maad_source_ipv4': sorted(self.maad_source_ipv4),
            'maad_destination_ipv4': sorted(self.maad_destination_ipv4),
            'netflow_rows': [dict(row) for row in self.netflow_rows()],
        }


def load_pipeline_v2_config(path: str | Path) -> dict:
    """Load the minimal v2 pipeline config file."""
    with open(path, 'r', encoding='utf-8') as handle:
        payload = json.load(handle)
    if not isinstance(payload, dict):
        raise ValueError('pipeline_v2 config must be a json object')
    inputs = payload.get('inputs')
    if not isinstance(inputs, list):
        raise ValueError("pipeline_v2 config must include an 'inputs' list")
    return payload


def process_pipeline_v2_config(conn: sqlite3.Connection, config: dict) -> None:
    """Process a v2 config, including canonical nfcapd tree inputs."""
    init_datasets_table(conn)
    for dataset in config.get('datasets', []):
        upsert_dataset_metadata(conn, dataset)

    maad_bin = config.get('maad_bin', DEFAULT_MAAD_BIN)
    maad_backend = str(config.get('maad_backend', 'subprocess'))
    maad_workers = int(config.get('maad_workers', 1))
    max_workers = int(config.get('max_workers', DEFAULT_MAX_WORKERS))
    run_maad = bool(config.get('run_maad', True))
    explicit_inputs = []

    for spec in config['inputs']:
        input_kind = str(spec['input_kind'])
        if input_kind == 'nfcapd_tree':
            process_nfcapd_tree_spec(
                conn,
                spec,
                maad_bin=maad_bin,
                maad_backend=maad_backend,
                maad_workers=maad_workers,
                max_workers=max_workers,
                run_maad=run_maad,
            )
        elif input_kind == 'csv_tree':
            process_csv_tree_spec(
                conn,
                spec,
                maad_bin=maad_bin,
                maad_backend=maad_backend,
                maad_workers=maad_workers,
                max_workers=max_workers,
                run_maad=run_maad,
            )
        else:
            explicit_inputs.append(spec)

    if explicit_inputs:
        process_input_specs(
            conn,
            explicit_inputs,
            maad_bin=maad_bin,
            maad_backend=maad_backend,
            maad_workers=maad_workers,
            max_workers=max_workers,
            run_maad=run_maad,
        )


def process_nfcapd_tree_spec(
    conn: sqlite3.Connection,
    spec: dict,
    *,
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
    max_workers: int,
    run_maad: bool,
) -> None:
    """Process a canonical nfcapd tree one day at a time."""
    root_path = Path(spec['root_path'])
    source_ids = [str(source_id) for source_id in spec['source_ids']]
    start_date = parse_config_date(str(spec['start_date']))
    end_date = (
        parse_config_date(str(spec['end_date']))
        if spec.get('end_date')
        else discover_latest_nfcapd_tree_day(root_path, source_ids)
    )
    zero_fill_gaps = bool(spec.get('zero_fill_gaps', True))
    source_bounds = discover_nfcapd_source_bounds(root_path, source_ids) if zero_fill_gaps else {}

    for day in iter_days(start_date, end_date):
        input_specs = discover_nfcapd_tree_specs(
            root_path,
            source_ids,
            day,
            source_bounds=source_bounds,
        )
        if not input_specs:
            LOGGER.info('No nfcapd files found for %s', day.strftime('%Y-%m-%d'))
            continue
        if all_tree_specs_processed(conn, input_specs):
            print(f"[pipeline_v2] Skip {day.strftime('%Y-%m-%d')}: {len(input_specs)} already processed")
            continue
        print(f"[pipeline_v2] Processing {day.strftime('%Y-%m-%d')}: {len(input_specs)} nfcapd files")
        process_input_specs(
            conn,
            input_specs,
            maad_bin=maad_bin,
            maad_backend=maad_backend,
            maad_workers=maad_workers,
            max_workers=max_workers,
            run_maad=run_maad,
        )
        print(f"[pipeline_v2] Complete {day.strftime('%Y-%m-%d')}: {len(input_specs)} nfcapd files")


def process_csv_tree_spec(
    conn: sqlite3.Connection,
    spec: dict,
    *,
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
    max_workers: int,
    run_maad: bool,
) -> None:
    """Process configured CSV files under one directory."""
    input_specs = discover_csv_specs(spec['root_path'], spec['mapping_path'])
    if not input_specs:
        LOGGER.info('No CSV files found under %s', spec['root_path'])
        return
    print(f"[pipeline_v2] Processing {len(input_specs)} CSV inputs")
    process_input_specs(
        conn,
        input_specs,
        maad_bin=maad_bin,
        maad_backend=maad_backend,
        maad_workers=maad_workers,
        max_workers=max_workers,
        run_maad=run_maad,
    )
    print(f"[pipeline_v2] Complete {len(input_specs)} CSV inputs")


def discover_nfcapd_tree_specs(
    root_path: str | Path,
    source_ids: list[str],
    day: datetime,
    *,
    source_bounds: dict[str, tuple[int, int]] | None = None,
) -> list[dict]:
    """Discover nfcapd files and bounded gap buckets under <root>/<source>/YYYY/MM/DD."""
    root = Path(root_path)
    specs = []
    for source_id in sorted(source_ids):
        day_dir = root / source_id / day.strftime('%Y') / day.strftime('%m') / day.strftime('%d')
        real_specs = {}
        if day_dir.is_dir():
            for path in sorted(day_dir.glob('nfcapd.*')):
                if not is_nfcapd_bucket_filename(path.name):
                    continue
                real_specs[parse_nfcapd_bucket_start(str(path))] = {
                    'input_kind': 'nfcapd',
                    'path': str(path),
                    'source_id': source_id,
                }
        if source_bounds is None or source_id not in source_bounds:
            specs.extend(real_specs[bucket_start] for bucket_start in sorted(real_specs))
            continue

        first_bucket, last_bucket = source_bounds[source_id]
        for bucket_start in iter_local_day_bucket_starts(day):
            spec = real_specs.get(bucket_start)
            if spec is not None:
                specs.append(spec)
            elif first_bucket < bucket_start < last_bucket:
                specs.append(build_nfcapd_gap_spec(root, source_id, bucket_start))
    return specs


def discover_nfcapd_source_bounds(
    root_path: str | Path,
    source_ids: list[str],
) -> dict[str, tuple[int, int]]:
    """Return first and last real nfcapd bucket per source."""
    root = Path(root_path)
    bounds: dict[str, tuple[int, int]] = {}
    for source_id in source_ids:
        bucket_starts = [
            parse_nfcapd_bucket_start(str(path))
            for path in iter_nfcapd_source_paths(root / source_id)
        ]
        if bucket_starts:
            bounds[source_id] = (min(bucket_starts), max(bucket_starts))
    return bounds


def iter_nfcapd_source_paths(source_root: Path) -> Iterable[Path]:
    """Yield canonical nfcapd files below one source root."""
    if not source_root.is_dir():
        return
    for year_dir in sorted(source_root.glob('????')):
        if not year_dir.is_dir():
            continue
        for month_dir in sorted(year_dir.glob('??')):
            if not month_dir.is_dir():
                continue
            for day_dir in sorted(month_dir.glob('??')):
                if not day_dir.is_dir():
                    continue
                for path in sorted(day_dir.glob('nfcapd.*')):
                    if is_nfcapd_bucket_filename(path.name):
                        yield path


def iter_local_day_bucket_starts(day: datetime) -> Iterable[int]:
    """Yield local-time 5m bucket starts for one calendar day."""
    current = day.replace(tzinfo=PIPELINE_TIMEZONE)
    end = current + timedelta(days=1)
    while current < end:
        yield int(current.timestamp())
        current += timedelta(seconds=FIVE_MINUTE_SECONDS)


def build_nfcapd_gap_spec(root: Path, source_id: str, bucket_start: int) -> dict:
    """Build an internal zero-fill spec for one missing nfcapd bucket."""
    timestamp = datetime.fromtimestamp(bucket_start, PIPELINE_TIMEZONE)
    return {
        'input_kind': 'nfcapd',
        'path': f"gap://nfcapd/{source_id}/{timestamp.strftime('%Y%m%d%H%M')}",
        'expected_path': str(
            root
            / source_id
            / timestamp.strftime('%Y')
            / timestamp.strftime('%m')
            / timestamp.strftime('%d')
            / f"nfcapd.{timestamp.strftime('%Y%m%d%H%M')}"
        ),
        'source_id': source_id,
        'bucket_start': bucket_start,
        'gap': True,
    }


def all_tree_specs_processed(conn: sqlite3.Connection, input_specs: list[dict]) -> bool:
    """Return true when every discovered file has processed v2 status."""
    if not input_specs:
        return False
    init_processed_inputs_v2_table(conn)
    locators = [spec['path'] for spec in input_specs]
    processed = set()
    for batch in chunked(locators, 900):
        placeholders = ','.join('?' for _ in batch)
        rows = conn.execute(
            f"""
            SELECT input_locator
            FROM processed_inputs_v2
            WHERE input_kind = 'nfcapd'
              AND status = 'processed'
              AND input_locator IN ({placeholders})
            """,
            batch,
        ).fetchall()
        processed.update(row[0] for row in rows)
    return processed == set(locators)


def chunked(values: list[str], size: int) -> Iterable[list[str]]:
    """Yield fixed-size chunks from values."""
    for index in range(0, len(values), size):
        yield values[index:index + size]


def discover_latest_nfcapd_tree_day(root_path: str | Path, source_ids: list[str]) -> datetime:
    """Return the latest day containing a canonical nfcapd file."""
    root = Path(root_path)
    latest: datetime | None = None
    for source_id in source_ids:
        source_root = root / source_id
        if not source_root.is_dir():
            continue
        for year_dir in sorted(source_root.glob('????')):
            if not year_dir.is_dir():
                continue
            for month_dir in sorted(year_dir.glob('??')):
                if not month_dir.is_dir():
                    continue
                for day_dir in sorted(month_dir.glob('??')):
                    if not day_dir.is_dir():
                        continue
                    try:
                        day = datetime.strptime(
                            f'{year_dir.name}/{month_dir.name}/{day_dir.name}',
                            '%Y/%m/%d',
                        )
                    except ValueError:
                        continue
                    if latest is None or day > latest:
                        latest = day

    if latest is None:
        raise ValueError(f'No nfcapd files found under {root}')
    return latest


def iter_days(start_date: datetime, end_date: datetime) -> Iterable[datetime]:
    """Yield inclusive calendar days."""
    if end_date < start_date:
        raise ValueError('end_date must be on or after start_date')
    current = start_date
    while current <= end_date:
        yield current
        current += timedelta(days=1)


def parse_config_date(raw_value: str) -> datetime:
    """Parse a YYYY-MM-DD date from config or CLI."""
    try:
        return datetime.strptime(raw_value, '%Y-%m-%d')
    except ValueError as error:
        raise ValueError(f'Invalid date {raw_value!r}; expected YYYY-MM-DD') from error


def build_dataset_tree_config(
    *,
    dataset_id: str,
    start_date: str,
    end_date: str | None = None,
    database_path: str | Path | None = None,
    maad_bin: str | Path = DEFAULT_MAAD_BIN,
    max_workers: int = DEFAULT_MAX_WORKERS,
) -> dict:
    """Build a v2 nfcapd_tree config from datasets.json."""
    from common import get_dataset_config, list_dataset_sources

    dataset = get_dataset_config(dataset_id)
    db_path = (
        Path(database_path)
        if database_path is not None
        else Path(__file__).resolve().parents[2] / 'data' / f'{dataset_id}-v2' / 'netflow.sqlite'
    )
    tree_input = {
        'input_kind': 'nfcapd_tree',
        'root_path': dataset['root_path'],
        'source_ids': list_dataset_sources(dataset_id),
        'start_date': start_date,
    }
    if end_date is not None:
        tree_input['end_date'] = end_date
    return {
        'database_path': str(db_path),
        'maad_bin': str(maad_bin),
        'max_workers': max_workers,
        'datasets': [dataset],
        'inputs': [tree_input],
    }


def process_input_specs(
    conn: sqlite3.Connection,
    input_specs: list[dict],
    *,
    maad_bin: str | Path = DEFAULT_MAAD_BIN,
    maad_backend: str = 'subprocess',
    maad_workers: int = 1,
    max_workers: int = DEFAULT_MAX_WORKERS,
    run_maad: bool = True,
) -> None:
    """Process explicit input specs into the v2 aggregate tables."""
    init_processed_inputs_v2_table(conn)
    init_netflow_stats_v2_table(conn)
    init_ip_stats_v2_table(conn)
    init_protocol_stats_v2_table(conn)
    init_maad_v2_tables(conn)

    if max_workers == 1 and should_stream_csv_input_specs(input_specs):
        process_csv_input_specs_streaming(
            conn,
            input_specs,
            maad_bin=maad_bin,
            maad_backend=maad_backend,
            maad_workers=maad_workers,
            run_maad=run_maad,
        )
        return

    if should_stream_nfcapd_input_specs(input_specs):
        process_nfcapd_input_specs_streaming_aggregates(
            conn,
            input_specs,
            maad_bin=maad_bin,
            maad_backend=maad_backend,
            maad_workers=maad_workers,
            max_workers=max_workers,
            run_maad=run_maad,
        )
        return

    tasks = [(spec, str(maad_bin), maad_backend, maad_workers, run_maad) for spec in input_specs]
    processed_buckets = []
    raw_buckets = []

    for payload in iter_input_payloads(tasks, max_workers):
        write_input_payload(conn, payload, mark_processed=False)
        processed_buckets.extend(payload['processed_buckets'])
        raw_buckets.extend(payload.get('raw_buckets', []))

    write_aggregate_rows(
        conn,
        raw_buckets,
        maad_bin,
        max_workers,
        maad_backend=maad_backend,
        maad_workers=maad_workers,
        run_maad=run_maad,
    )
    with conn:
        mark_processed_buckets(conn, processed_buckets)


def should_stream_nfcapd_input_specs(input_specs: list[dict]) -> bool:
    """Return true when every input spec is a native nfcapd bucket."""
    return bool(input_specs) and all(str(spec['input_kind']) == 'nfcapd' for spec in input_specs)


def process_nfcapd_input_specs_streaming_aggregates(
    conn: sqlite3.Connection,
    input_specs: list[dict],
    *,
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
    max_workers: int,
    run_maad: bool,
) -> None:
    """Process nfcapd inputs without retaining every raw 5m address payload."""
    tasks = [(spec, str(maad_bin), maad_backend, maad_workers, run_maad) for spec in input_specs]
    processed_buckets = []
    aggregate_buckets: dict[tuple[str, str, int], dict] = {}

    for payload in iter_input_payloads(tasks, max_workers):
        write_input_payload(conn, payload, mark_processed=False)
        processed_buckets.extend(payload['processed_buckets'])
        for raw_bucket in payload.get('raw_buckets', []):
            add_raw_bucket_to_streaming_aggregates(aggregate_buckets, raw_bucket)

    aggregate_workers = aggregate_maad_worker_count(max(max_workers, maad_workers))
    flush_streaming_aggregate_buckets(
        conn,
        aggregate_buckets,
        list(aggregate_buckets),
        maad_bin,
        maad_backend,
        aggregate_workers,
        run_maad,
    )
    with conn:
        mark_processed_buckets(conn, processed_buckets)


def should_stream_csv_input_specs(input_specs: list[dict]) -> bool:
    """Return true when CSV specs are safe for bounded ordered streaming."""
    if not input_specs or not all(str(spec['input_kind']) == 'csv' for spec in input_specs):
        return False
    return all(
        load_csv_source_config(spec['mapping_path']).input_order == 'timestamp_ascending'
        for spec in input_specs
    )


def process_csv_input_specs_streaming(
    conn: sqlite3.Connection,
    input_specs: list[dict],
    *,
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
    run_maad: bool,
) -> None:
    """Process CSV inputs with bounded active bucket memory and visible progress."""
    processed_buckets = []
    aggregate_buckets = {}
    for spec in input_specs:
        config = load_csv_source_config(spec['mapping_path'])
        input_locator = str(spec['path'])
        if csv_input_fully_processed(conn, input_locator):
            print(f'[pipeline_v2] Skip CSV input already processed: {input_locator}')
            continue
        if should_accumulate_csv_with_arrow(config):
            process_csv_input_spec_arrow_streaming(
                conn,
                spec,
                config,
                maad_bin=maad_bin,
                maad_backend=maad_backend,
                maad_workers=maad_workers,
                run_maad=run_maad,
                processed_buckets=processed_buckets,
                aggregate_buckets=aggregate_buckets,
            )
            continue

        if should_accumulate_csv_values_directly(config):
            process_csv_input_spec_values_streaming(
                conn,
                spec,
                config,
                maad_bin=maad_bin,
                maad_backend=maad_backend,
                maad_workers=maad_workers,
                run_maad=run_maad,
                processed_buckets=processed_buckets,
                aggregate_buckets=aggregate_buckets,
            )
            continue

        print(f'[pipeline_v2] CSV start: {input_locator}')
        active_buckets: dict[tuple[str, int, int], BucketAccumulator] = {}
        max_bucket_start: int | None = None
        rows_seen = 0

        for row in iter_input_rows(spec):
            rows_seen += 1
            max_bucket_start = (
                row.bucket_start
                if max_bucket_start is None
                else max(max_bucket_start, row.bucket_start)
            )
            cutoff = max_bucket_start - (config.out_of_order_lag_buckets * 300)
            if row.bucket_start < cutoff:
                raise ValueError(
                    f'CSV input is not ordered enough for streaming: {input_locator} row bucket '
                    f'{row.bucket_start} arrived after flush cutoff {cutoff}. Set '
                    '"input_order": "unsorted" to use full-file aggregation.'
                )
            add_row_to_bucket(active_buckets, row)

            if rows_seen % CSV_STREAM_PROGRESS_ROWS == 0:
                print(
                    f'[pipeline_v2] CSV rows={rows_seen} active_buckets={len(active_buckets)} '
                    f'input={input_locator}'
                )

            ready_keys = [key for key in active_buckets if key[1] < cutoff]
            if ready_keys:
                flush_csv_buckets(
                    conn,
                    spec,
                    active_buckets,
                    ready_keys,
                    maad_bin,
                    maad_backend,
                    maad_workers,
                    run_maad,
                    processed_buckets,
                    aggregate_buckets,
                    cutoff,
                )

        flush_csv_buckets(
            conn,
            spec,
            active_buckets,
            list(active_buckets),
            maad_bin,
            maad_backend,
            maad_workers,
            run_maad,
            processed_buckets,
            aggregate_buckets,
            max_bucket_start if max_bucket_start is not None else 0,
        )
        print(
            f'[pipeline_v2] CSV complete: rows={rows_seen} buckets={len(processed_buckets)} '
            f'input={input_locator}'
        )
    flush_streaming_aggregate_buckets(
        conn,
        aggregate_buckets,
        list(aggregate_buckets),
        maad_bin,
        maad_backend,
        maad_workers,
        run_maad,
    )
    with conn:
        mark_processed_buckets(conn, processed_buckets)


def should_accumulate_csv_values_directly(config: CsvSourceConfig) -> bool:
    """Return true when CSV rows can be accumulated from indexed values."""
    return getattr(config, 'has_header', True) is False and getattr(config, 'fieldnames', None) is not None


def csv_input_fully_processed(conn: sqlite3.Connection, input_locator: str) -> bool:
    """Return true when a CSV input has only processed bucket records."""
    init_processed_inputs_v2_table(conn)
    rows = conn.execute(
        """
        SELECT status, COUNT(*)
        FROM processed_inputs_v2
        WHERE input_kind = 'csv' AND input_locator = ?
        GROUP BY status
        """,
        (input_locator,),
    ).fetchall()
    if not rows:
        return False
    status_counts = {status: count for status, count in rows}
    return status_counts.get('processed', 0) > 0 and sum(
        count for status, count in status_counts.items() if status != 'processed'
    ) == 0


def should_accumulate_csv_with_arrow(config: CsvSourceConfig) -> bool:
    """Return true when PyArrow can vectorize the mapped CSV stream."""
    if getattr(config, 'has_header', True) is not False or getattr(config, 'fieldnames', None) is None:
        return False
    if config.delimiter != ',' or config.source_id_value is None:
        return False
    if config.timestamp_format != 'datetime' or config.datetime_format != '%Y-%m-%d %H:%M:%S':
        return False
    required_columns = {'src_ip', 'dst_ip', 'protocol', 'packets', 'bytes'}
    return required_columns.issubset(config.columns)


def arrow_ipv4_address_mask(values):
    """Return a PyArrow boolean mask for dotted-quad IPv4 strings."""
    import pyarrow.compute as pc

    return pc.match_substring_regex(values, ARROW_IPV4_REGEX)


def arrow_ipv4_pair_mask(batch, src_column: str, dst_column: str):
    """Return a PyArrow mask for rows with two valid IPv4 endpoints."""
    import pyarrow.compute as pc

    return pc.and_(
        arrow_ipv4_address_mask(batch[src_column]),
        arrow_ipv4_address_mask(batch[dst_column]),
    )


def arrow_valid_ip_pair_mask(batch, src_column: str, dst_column: str):
    """Return a PyArrow mask for rows with same-family endpoints."""
    import pyarrow.compute as pc

    src_has_colon = pc.match_substring(batch[src_column], ':')
    dst_has_colon = pc.match_substring(batch[dst_column], ':')
    ipv4_pair = arrow_ipv4_pair_mask(batch, src_column, dst_column)
    ipv6_pair = pc.and_(src_has_colon, dst_has_colon)
    return pc.or_(ipv4_pair, ipv6_pair)


def process_csv_input_spec_arrow_streaming(
    conn: sqlite3.Connection,
    spec: dict,
    config: CsvSourceConfig,
    *,
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
    run_maad: bool,
    processed_buckets: list[dict],
    aggregate_buckets: dict[tuple[str, str, int], dict],
) -> None:
    """Stream a headerless CSV input through PyArrow grouped batches."""
    input_locator = str(spec['path'])
    active_arrow_buckets: dict[int, list] = {}
    max_bucket_start: int | None = None
    rows_seen = 0

    print(f'[pipeline_v2] CSV start: {input_locator}')
    for batch in iter_csv_arrow_batches(input_locator, config):
        rows_seen += batch.num_rows
        max_bucket_start = merge_arrow_batch_into_table_buckets(
            active_arrow_buckets,
            batch,
            config,
            max_bucket_start,
        )
        if max_bucket_start is None:
            continue

        cutoff = max_bucket_start - (config.out_of_order_lag_buckets * 300)
        if rows_seen % CSV_STREAM_PROGRESS_ROWS < batch.num_rows:
            print(
                f'[pipeline_v2] CSV rows={rows_seen} active_buckets={len(active_arrow_buckets)} '
                f'input={input_locator}'
            )

        ready_bucket_starts = [bucket_start for bucket_start in active_arrow_buckets if bucket_start < cutoff]
        if run_maad and len(ready_bucket_starts) < CSV_MAAD_BATCH_BUCKETS:
            continue
        if ready_bucket_starts:
            flush_csv_arrow_buckets(
                conn,
                spec,
                active_arrow_buckets,
                ready_bucket_starts,
                config,
                maad_bin,
                maad_backend,
                maad_workers,
                run_maad,
                processed_buckets,
                aggregate_buckets,
                cutoff,
            )

    flush_csv_arrow_buckets(
        conn,
        spec,
        active_arrow_buckets,
        list(active_arrow_buckets),
        config,
        maad_bin,
        maad_backend,
        maad_workers,
        run_maad,
        processed_buckets,
        aggregate_buckets,
        max_bucket_start if max_bucket_start is not None else 0,
    )
    print(
        f'[pipeline_v2] CSV complete: rows={rows_seen} buckets={len(processed_buckets)} '
        f'input={input_locator}'
    )


def iter_csv_arrow_batches(input_locator: str, config: CsvSourceConfig):
    """Yield PyArrow record batches for one configured headerless CSV input."""
    import pyarrow as pa
    import pyarrow.csv as arrow_csv

    include_columns = [
        config.columns['time_end'],
        config.columns['src_ip'],
        config.columns['dst_ip'],
        config.columns['protocol'],
        config.columns['packets'],
        config.columns['bytes'],
    ]
    column_types = {column_name: pa.string() for column_name in include_columns}

    def invalid_row_handler(_row):
        return 'skip' if config.skip_bad_column_count else 'error'

    read_options = arrow_csv.ReadOptions(
        column_names=config.fieldnames,
        block_size=CSV_ARROW_BLOCK_BYTES,
    )
    parse_options = arrow_csv.ParseOptions(
        delimiter=config.delimiter,
        invalid_row_handler=invalid_row_handler,
    )
    convert_options = arrow_csv.ConvertOptions(
        include_columns=include_columns,
        column_types=column_types,
    )

    input_path = Path(input_locator)
    if is_tar_archive(input_path):
        with tarfile.open(input_path, mode='r:*') as archive:
            for member in archive:
                if not member.isfile():
                    continue
                if config.archive_member_contains and config.archive_member_contains not in member.name:
                    continue
                extracted = archive.extractfile(member)
                if extracted is None:
                    continue
                with extracted:
                    yield from iter_csv_arrow_reader_batches(
                        extracted,
                        read_options,
                        parse_options,
                        convert_options,
                    )
        return

    with open(input_path, 'rb') as handle:
        yield from iter_csv_arrow_reader_batches(
            handle,
            read_options,
            parse_options,
            convert_options,
        )


def iter_csv_arrow_reader_batches(handle, read_options, parse_options, convert_options):
    """Yield batches from a PyArrow CSV reader and close it deterministically."""
    import pyarrow.csv as arrow_csv

    reader = arrow_csv.open_csv(
        handle,
        read_options=read_options,
        parse_options=parse_options,
        convert_options=convert_options,
    )
    try:
        while True:
            try:
                yield reader.read_next_batch()
            except StopIteration:
                break
    finally:
        reader.close()


def merge_arrow_batch_into_table_buckets(
    active_arrow_buckets: dict[int, list],
    batch,
    config: CsvSourceConfig,
    max_bucket_start: int | None,
) -> int | None:
    """Split one PyArrow batch into active 5-minute table chunks."""
    import pyarrow as pa
    import pyarrow.compute as pc

    time_column = config.columns['time_end']
    src_column = config.columns['src_ip']
    dst_column = config.columns['dst_ip']
    protocol_column = config.columns['protocol']
    packets_column = config.columns['packets']
    bytes_column = config.columns['bytes']
    batch = filter_arrow_valid_flow_rows(
        batch,
        config=config,
        time_column=time_column,
        src_column=src_column,
        dst_column=dst_column,
        protocol_column=protocol_column,
        packets_column=packets_column,
        bytes_column=bytes_column,
    )
    if batch.num_rows == 0:
        return max_bucket_start

    minute = pc.utf8_slice_codeunits(batch[time_column], 0, 16)
    src_is_ipv6 = pc.match_substring(batch[src_column], ':')
    ip_version = pc.if_else(src_is_ipv6, pa.scalar(6, pa.int8()), pa.scalar(4, pa.int8()))
    table = pa.table(
        {
            'minute': minute,
            'ip_version': ip_version,
            'src_ip': batch[src_column],
            'dst_ip': batch[dst_column],
            'protocol': batch[protocol_column],
            'packets': pc.cast(batch[packets_column], pa.int64()),
            'bytes': pc.cast(batch[bytes_column], pa.int64()),
        }
    )

    for raw_minute in pc.unique(minute).to_pylist():
        bucket_start = parse_arrow_minute_bucket(raw_minute, config)
        max_bucket_start = (
            bucket_start if max_bucket_start is None else max(max_bucket_start, bucket_start)
        )
        minute_table = table.filter(pc.equal(table['minute'], raw_minute)).drop(['minute'])
        active_arrow_buckets.setdefault(bucket_start, []).append(minute_table)
    return max_bucket_start


def filter_arrow_valid_flow_rows(
    batch,
    *,
    config: CsvSourceConfig,
    time_column: str,
    src_column: str,
    dst_column: str,
    protocol_column: str,
    packets_column: str,
    bytes_column: str,
):
    """Drop malformed high-volume CSV rows before vectorized casts."""
    import pyarrow as pa
    import pyarrow.compute as pc

    mask = pc.and_(
        pc.match_substring_regex(
            batch[time_column],
            r'^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$',
        ),
        pc.match_substring_regex(batch[packets_column], r'^\d+$'),
    )
    mask = pc.and_(mask, pc.match_substring_regex(batch[bytes_column], r'^\d+$'))
    protocol_numeric_or_empty = pc.match_substring_regex(batch[protocol_column], r'^\d*$')
    protocol_known_name = pc.is_in(
        batch[protocol_column],
        value_set=pa.array(sorted(config.protocol_map)),
    )
    mask = pc.and_(mask, pc.or_(protocol_numeric_or_empty, protocol_known_name))
    mask = pc.and_(mask, pc.not_equal(batch[src_column], ''))
    mask = pc.and_(mask, pc.not_equal(batch[dst_column], ''))
    mask = pc.and_(mask, arrow_valid_ip_pair_mask(batch, src_column, dst_column))
    return batch.filter(mask)


def flush_csv_arrow_buckets(
    conn: sqlite3.Connection,
    spec: dict,
    active_arrow_buckets: dict[int, list],
    bucket_starts: list[int],
    config: CsvSourceConfig,
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
    run_maad: bool,
    processed_buckets: list[dict],
    aggregate_buckets: dict[tuple[str, str, int], dict],
    aggregate_cutoff: int,
) -> None:
    """Build and flush completed PyArrow-backed CSV buckets."""
    if not bucket_starts:
        return
    assert config.source_id_value is not None
    bucket_values = [
        build_bucket_accumulator_from_arrow_tables(
            source_id=config.source_id_value,
            bucket_start=bucket_start,
            tables=active_arrow_buckets.pop(bucket_start),
            config=config,
        )
        for bucket_start in sorted(bucket_starts)
    ]
    flush_csv_bucket_values(
        conn,
        spec,
        bucket_values,
        maad_bin,
        maad_backend,
        maad_workers,
        run_maad,
        processed_buckets,
        aggregate_buckets,
        aggregate_cutoff,
    )


def build_bucket_accumulator_from_arrow_tables(
    *,
    source_id: str,
    bucket_start: int,
    tables: list,
    config: CsvSourceConfig,
) -> BucketAccumulator:
    """Build a BucketAccumulator from Arrow chunks for one 5-minute bucket."""
    import pyarrow as pa
    import pyarrow.compute as pc

    table = pa.concat_tables(tables)
    bucket = BucketAccumulator(
        source_id=source_id,
        bucket_start=bucket_start,
        bucket_end=bucket_start + 300,
    )

    netflow_groups = table.group_by(['ip_version', 'protocol']).aggregate(
        [('protocol', 'count'), ('packets', 'sum'), ('bytes', 'sum')]
    )
    for row in netflow_groups.to_pylist():
        bucket.add_netflow_metrics(
            ip_version=int(row['ip_version']),
            protocol=parse_arrow_protocol(row['protocol'], config),
            flows=int(row['protocol_count']),
            packets=int(row['packets_sum']),
            bytes_count=int(row['bytes_sum']),
        )

    for ip_version in (4, 6):
        family = table.filter(pc.equal(table['ip_version'], ip_version))
        if family.num_rows == 0:
            continue
        source_addresses = set(pc.unique(family['src_ip']).to_pylist())
        destination_addresses = set(pc.unique(family['dst_ip']).to_pylist())
        if ip_version == 4:
            bucket.source_ipv4.update(source_addresses)
            bucket.destination_ipv4.update(destination_addresses)
            bucket.maad_source_ipv4.update(source_addresses)
            bucket.maad_destination_ipv4.update(destination_addresses)
        else:
            bucket.source_ipv6.update(source_addresses)
            bucket.destination_ipv6.update(destination_addresses)

    return bucket


def merge_arrow_batch_into_buckets(
    active_buckets: dict[tuple[str, int, int], BucketAccumulator],
    batch,
    config: CsvSourceConfig,
    max_bucket_start: int | None,
) -> int | None:
    """Merge one vectorized CSV batch into active bucket state."""
    import pyarrow as pa
    import pyarrow.compute as pc

    time_column = config.columns['time_end']
    src_column = config.columns['src_ip']
    dst_column = config.columns['dst_ip']
    protocol_column = config.columns['protocol']
    packets_column = config.columns['packets']
    bytes_column = config.columns['bytes']

    minute = pc.utf8_slice_codeunits(batch[time_column], 0, 16)
    src_is_ipv6 = pc.match_substring(batch[src_column], ':')
    ip_version = pc.if_else(src_is_ipv6, pa.scalar(6, pa.int8()), pa.scalar(4, pa.int8()))
    packets = pc.cast(batch[packets_column], pa.int64())
    bytes_count = pc.cast(batch[bytes_column], pa.int64())
    table = pa.table(
        {
            'minute': minute,
            'ip_version': ip_version,
            'src_ip': batch[src_column],
            'dst_ip': batch[dst_column],
            'protocol': batch[protocol_column],
            'packets': packets,
            'bytes': bytes_count,
        }
    )

    max_bucket_start = merge_arrow_netflow_groups(active_buckets, table, config, max_bucket_start)
    merge_arrow_unique_ip_groups(active_buckets, table, config, 'src_ip')
    merge_arrow_unique_ip_groups(active_buckets, table, config, 'dst_ip')
    return max_bucket_start


def merge_arrow_netflow_groups(
    active_buckets: dict[tuple[str, int, int], BucketAccumulator],
    table,
    config: CsvSourceConfig,
    max_bucket_start: int | None,
) -> int | None:
    """Merge grouped netflow metrics from a PyArrow table."""
    grouped = table.group_by(['minute', 'ip_version', 'protocol']).aggregate(
        [('protocol', 'count'), ('packets', 'sum'), ('bytes', 'sum')]
    )
    for row in grouped.to_pylist():
        bucket_start = parse_arrow_minute_bucket(row['minute'], config)
        max_bucket_start = (
            bucket_start if max_bucket_start is None else max(max_bucket_start, bucket_start)
        )
        protocol = parse_arrow_protocol(row['protocol'], config)
        bucket = get_primitive_bucket(
            active_buckets,
            config.source_id_value,
            bucket_start,
        )
        bucket.add_netflow_metrics(
            ip_version=int(row['ip_version']),
            protocol=protocol,
            flows=int(row['protocol_count']),
            packets=int(row['packets_sum']),
            bytes_count=int(row['bytes_sum']),
        )
    return max_bucket_start


def merge_arrow_unique_ip_groups(
    active_buckets: dict[tuple[str, int, int], BucketAccumulator],
    table,
    config: CsvSourceConfig,
    column_name: str,
) -> None:
    """Merge grouped unique endpoint addresses from a PyArrow table."""
    grouped = table.group_by(['minute', 'ip_version', column_name]).aggregate([(column_name, 'count')])
    for row in grouped.to_pylist():
        bucket = get_primitive_bucket(
            active_buckets,
            config.source_id_value,
            parse_arrow_minute_bucket(row['minute'], config),
        )
        address = row[column_name]
        if int(row['ip_version']) == 4:
            if column_name == 'src_ip':
                bucket.source_ipv4.add(address)
                bucket.maad_source_ipv4.add(address)
            else:
                bucket.destination_ipv4.add(address)
                bucket.maad_destination_ipv4.add(address)
        else:
            if column_name == 'src_ip':
                bucket.source_ipv6.add(address)
            else:
                bucket.destination_ipv6.add(address)


def get_primitive_bucket(
    buckets: dict[tuple[str, int, int], BucketAccumulator],
    source_id: str,
    bucket_start: int,
) -> BucketAccumulator:
    """Return an active bucket for primitive aggregate paths."""
    bucket_end = bucket_start + 300
    key = (source_id, bucket_start, bucket_end)
    return buckets.setdefault(
        key,
        BucketAccumulator(
            source_id=source_id,
            bucket_start=bucket_start,
            bucket_end=bucket_end,
        ),
    )


def parse_arrow_minute_bucket(raw_minute: str, config: CsvSourceConfig) -> int:
    """Parse a YYYY-MM-DD HH:MM minute string to a configured 5-minute bucket."""
    raw_text = f'{raw_minute}:00'
    return parse_datetime_5m_bucket(
        raw_text,
        config.timestamp_timezone,
        config.datetime_format,
    )


def parse_arrow_protocol(raw_value: str, config: CsvSourceConfig) -> int:
    """Parse a protocol value from PyArrow grouped output."""
    raw_text = raw_value.strip()
    if raw_text == '':
        return 0
    protocol = config.protocol_map.get(raw_text.upper())
    if protocol is not None:
        return protocol
    try:
        return int(raw_text)
    except ValueError as error:
        raise CsvSourceConfigError(
            f"Invalid protocol value '{raw_text}' for column '{config.columns['protocol']}'."
        ) from error


def process_csv_input_spec_values_streaming(
    conn: sqlite3.Connection,
    spec: dict,
    config: CsvSourceConfig,
    *,
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
    run_maad: bool,
    processed_buckets: list[dict],
    aggregate_buckets: dict[tuple[str, str, int], dict],
) -> None:
    """Stream a headerless CSV input directly into bucket accumulators."""
    input_locator = str(spec['path'])
    field_indexes = build_field_indexes(config, input_locator)
    active_buckets: dict[tuple[str, int, int], BucketAccumulator] = {}
    max_bucket_start: int | None = None
    rows_seen = 0

    print(f'[pipeline_v2] CSV start: {input_locator}')
    for csv_row in iter_headerless_csv_values(input_locator, config):
        rows_seen += 1
        try:
            bucket_start = resolve_csv_value_bucket_start(csv_row.values, config, field_indexes)
            max_bucket_start = (
                bucket_start if max_bucket_start is None else max(max_bucket_start, bucket_start)
            )
            cutoff = max_bucket_start - (config.out_of_order_lag_buckets * 300)
            if bucket_start < cutoff:
                raise ValueError(
                    f'CSV input is not ordered enough for streaming: {input_locator} row bucket '
                    f'{bucket_start} arrived after flush cutoff {cutoff}. Set '
                    '"input_order": "unsorted" to use full-file aggregation.'
                )
            add_csv_values_to_bucket(active_buckets, csv_row.values, config, field_indexes, bucket_start)
        except CsvSourceConfigError as error:
            raise CsvSourceConfigError(
                f'{csv_row.locator}:{csv_row.line_number}: {error}'
            ) from error

        if rows_seen % CSV_STREAM_PROGRESS_ROWS == 0:
            print(
                f'[pipeline_v2] CSV rows={rows_seen} active_buckets={len(active_buckets)} '
                f'input={input_locator}'
            )

        ready_keys = [key for key in active_buckets if key[1] < cutoff]
        if ready_keys:
            flush_csv_buckets(
                conn,
                spec,
                active_buckets,
                ready_keys,
                maad_bin,
                maad_backend,
                maad_workers,
                run_maad,
                processed_buckets,
                aggregate_buckets,
                cutoff,
            )

    flush_csv_buckets(
        conn,
        spec,
        active_buckets,
        list(active_buckets),
        maad_bin,
        maad_backend,
        maad_workers,
        run_maad,
        processed_buckets,
        aggregate_buckets,
        max_bucket_start if max_bucket_start is not None else 0,
    )
    print(
        f'[pipeline_v2] CSV complete: rows={rows_seen} buckets={len(processed_buckets)} '
        f'input={input_locator}'
    )


def resolve_csv_value_bucket_start(
    values: list[str],
    config: CsvSourceConfig,
    field_indexes: dict[str, int],
) -> int:
    """Resolve a CSV value row timestamp directly to its 5-minute bucket."""
    for logical_key in TIMESTAMP_KEYS:
        column_name = config.columns.get(logical_key)
        if column_name is None:
            continue
        raw_value = values[field_indexes[column_name]].strip()
        if raw_value == '':
            continue
        if config.timestamp_format == 'datetime':
            return parse_datetime_5m_bucket(
                raw_value,
                config.timestamp_timezone,
                config.datetime_format,
            )
        unix_ts = parse_timestamp(
            raw_value,
            config.timestamp_format,
            config.timestamp_timezone,
            config.datetime_format,
        )
        return unix_ts - (unix_ts % 300)

    raise CsvSourceConfigError(
        'CSV row did not contain any usable timestamp value for the configured precedence.'
    )


@lru_cache(maxsize=200_000)
def parse_datetime_5m_bucket(raw_text: str, timestamp_timezone: str, datetime_format: str) -> int:
    """Parse a datetime string directly to a 5-minute bucket start."""
    if datetime_format == '%Y-%m-%d %H:%M:%S' and len(raw_text) >= 19:
        try:
            minute = int(raw_text[14:16])
            floored_minute = minute - (minute % 5)
            parsed = datetime(
                int(raw_text[0:4]),
                int(raw_text[5:7]),
                int(raw_text[8:10]),
                int(raw_text[11:13]),
                floored_minute,
                tzinfo=ZoneInfo(timestamp_timezone),
            )
        except ValueError as error:
            raise CsvSourceConfigError(f"Invalid timestamp value '{raw_text}'.") from error
        return int(parsed.timestamp())

    unix_ts = parse_timestamp(raw_text, 'datetime', timestamp_timezone, datetime_format)
    return unix_ts - (unix_ts % 300)


def add_csv_values_to_bucket(
    buckets: dict[tuple[str, int, int], BucketAccumulator],
    values: list[str],
    config: CsvSourceConfig,
    field_indexes: dict[str, int],
    bucket_start: int,
) -> None:
    """Accumulate one indexed CSV row into a bucket map."""
    bucket_end = bucket_start + 300
    source_id = resolve_csv_value_source_id(values, config, field_indexes)
    src_ip = require_csv_value(values, field_indexes[config.columns['src_ip']], config.columns['src_ip'])
    dst_ip = require_csv_value(values, field_indexes[config.columns['dst_ip']], config.columns['dst_ip'])
    ip_version = infer_ip_version(src_ip, dst_ip)
    protocol = extract_csv_value_protocol(values, config, field_indexes)
    packets = extract_csv_value_int(values, config, field_indexes, 'packets')
    bytes_count = extract_csv_value_int(values, config, field_indexes, 'bytes')

    key = (source_id, bucket_start, bucket_end)
    bucket = buckets.setdefault(
        key,
        BucketAccumulator(
            source_id=source_id,
            bucket_start=bucket_start,
            bucket_end=bucket_end,
        ),
    )
    bucket.add_flow(
        ip_version=ip_version,
        src_ip=src_ip,
        dst_ip=dst_ip,
        protocol=protocol,
        packets=packets,
        bytes_count=bytes_count,
    )


def resolve_csv_value_source_id(
    values: list[str],
    config: CsvSourceConfig,
    field_indexes: dict[str, int],
) -> str:
    """Resolve source_id from indexed CSV values."""
    if config.source_id_value is not None:
        return config.source_id_value
    assert config.source_id_column is not None
    return require_csv_value(values, field_indexes[config.source_id_column], config.source_id_column)


def require_csv_value(values: list[str], index: int, column_name: str) -> str:
    """Return a stripped required CSV value."""
    value = values[index].strip()
    if value == '':
        raise CsvSourceConfigError(f"CSV row is missing required value for column '{column_name}'.")
    return value


def extract_csv_value_int(
    values: list[str],
    config: CsvSourceConfig,
    field_indexes: dict[str, int],
    logical_key: str,
) -> int:
    """Extract an integer from indexed CSV values."""
    column_name = config.columns.get(logical_key)
    if column_name is None:
        return 0
    raw_text = values[field_indexes[column_name]].strip()
    if raw_text == '':
        return 0
    try:
        return int(raw_text)
    except ValueError as error:
        raise CsvSourceConfigError(
            f"Invalid integer value '{raw_text}' for column '{column_name}'."
        ) from error


def extract_csv_value_protocol(
    values: list[str],
    config: CsvSourceConfig,
    field_indexes: dict[str, int],
) -> int:
    """Extract protocol from indexed CSV values."""
    column_name = config.columns.get('protocol')
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


def flush_csv_buckets(
    conn: sqlite3.Connection,
    spec: dict,
    active_buckets: dict[tuple[str, int, int], BucketAccumulator],
    keys: list[tuple[str, int, int]],
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
    run_maad: bool,
    processed_buckets: list[dict],
    aggregate_buckets: dict[tuple[str, str, int], dict],
    aggregate_cutoff: int,
) -> None:
    """Flush selected active CSV buckets to SQLite."""
    if not keys:
        return
    bucket_values = [active_buckets.pop(key) for key in sorted(keys)]
    flush_csv_bucket_values(
        conn,
        spec,
        bucket_values,
        maad_bin,
        maad_backend,
        maad_workers,
        run_maad,
        processed_buckets,
        aggregate_buckets,
        aggregate_cutoff,
    )


def flush_csv_bucket_values(
    conn: sqlite3.Connection,
    spec: dict,
    bucket_values: list[BucketAccumulator],
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
    run_maad: bool,
    processed_buckets: list[dict],
    aggregate_buckets: dict[tuple[str, str, int], dict],
    aggregate_cutoff: int,
) -> None:
    """Flush completed CSV bucket accumulators to SQLite."""
    if not bucket_values:
        return
    bucket_values = skip_processed_csv_bucket_values(conn, str(spec['path']), bucket_values)
    if not bucket_values:
        return
    payload = build_bucket_payload_from_values(
        input_kind='csv',
        input_locator=str(spec['path']),
        bucket_values=bucket_values,
        maad_bin=maad_bin,
        maad_backend=maad_backend,
        maad_workers=maad_workers,
        run_maad=run_maad,
    )
    write_input_payload(conn, payload, mark_processed=False)
    processed_buckets.extend(payload['processed_buckets'])
    for raw_bucket in payload['raw_buckets']:
        add_raw_bucket_to_streaming_aggregates(aggregate_buckets, raw_bucket)
    ready_aggregate_keys = [
        key for key, bucket in aggregate_buckets.items() if bucket['bucket_end'] <= aggregate_cutoff
    ]
    flush_streaming_aggregate_buckets(
        conn,
        aggregate_buckets,
        ready_aggregate_keys,
        maad_bin,
        maad_backend,
        maad_workers,
        run_maad,
    )
    mark_csv_buckets_with_flushed_aggregates(conn, processed_buckets, aggregate_cutoff)


def skip_processed_csv_bucket_values(
    conn: sqlite3.Connection,
    input_locator: str,
    bucket_values: list[BucketAccumulator],
) -> list[BucketAccumulator]:
    """Drop CSV buckets already marked processed during a retry."""
    processed_keys = {
        (row[0], row[1])
        for row in conn.execute(
            """
            SELECT source_id, bucket_start
            FROM processed_inputs_v2
            WHERE input_kind = 'csv'
              AND input_locator = ?
              AND status = 'processed'
            """,
            (input_locator,),
        ).fetchall()
    }
    if not processed_keys:
        return bucket_values
    return [
        bucket
        for bucket in bucket_values
        if (bucket.source_id, bucket.bucket_start) not in processed_keys
    ]


def mark_csv_buckets_with_flushed_aggregates(
    conn: sqlite3.Connection,
    processed_buckets: list[dict],
    aggregate_cutoff: int,
) -> None:
    """Mark CSV buckets processed after all enclosing aggregate buckets flushed."""
    ready = ready_csv_buckets_for_processed_mark(conn, aggregate_cutoff)
    if not ready:
        return
    ready_ids = {
        (bucket['input_kind'], bucket['input_locator'], bucket['source_id'], bucket['bucket_start'])
        for bucket in ready
    }
    with conn:
        mark_processed_buckets(conn, ready)
    processed_buckets[:] = [
        bucket
        for bucket in processed_buckets
        if (
            bucket['input_kind'],
            bucket['input_locator'],
            bucket['source_id'],
            bucket['bucket_start'],
        )
        not in ready_ids
    ]


def ready_csv_buckets_for_processed_mark(conn: sqlite3.Connection, aggregate_cutoff: int) -> list[dict]:
    """Return pending CSV buckets whose aggregate outputs are closed."""
    rows = conn.execute(
        """
        SELECT input_kind, input_locator, source_id, bucket_start, bucket_end
        FROM processed_inputs_v2
        WHERE input_kind = 'csv' AND status != 'processed'
        ORDER BY bucket_start, input_locator, source_id
        """
    ).fetchall()
    return [
        {
            'input_kind': row[0],
            'input_locator': row[1],
            'source_id': row[2],
            'bucket_start': row[3],
            'bucket_end': row[4],
        }
        for row in rows
        if csv_bucket_aggregate_outputs_flushed(int(row[3]), aggregate_cutoff)
    ]


def csv_bucket_aggregate_outputs_flushed(bucket_start: int, aggregate_cutoff: int) -> bool:
    """Return true when every aggregate granularity containing a 5m bucket is closed."""
    return all(
        next_bucket_start(floor_bucket_start(bucket_start, seconds), seconds) <= aggregate_cutoff
        for _, seconds in AGGREGATE_GRANULARITY_SECONDS
    )


def build_bucket_payload_from_values(
    *,
    input_kind: str,
    input_locator: str,
    bucket_values: list[BucketAccumulator],
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
    run_maad: bool,
) -> dict:
    """Build DB insert payloads from completed bucket accumulators."""
    return {
        'processed_buckets': [
            {
                'input_kind': input_kind,
                'input_locator': input_locator,
                'source_id': bucket.source_id,
                'bucket_start': bucket.bucket_start,
                'bucket_end': bucket.bucket_end,
            }
            for bucket in bucket_values
        ],
        'netflow_rows': [row for bucket in bucket_values for row in bucket.netflow_rows()],
        'ip_rows': [bucket.ip_row() for bucket in bucket_values],
        'protocol_rows': [bucket.protocol_row() for bucket in bucket_values],
        'maad_rows': build_maad_payload_rows(
            {
                (bucket.source_id, bucket.bucket_start, bucket.bucket_end): bucket
                for bucket in bucket_values
            },
            maad_bin,
            maad_backend,
            maad_workers,
        )
        if run_maad
        else [],
        'raw_buckets': [bucket.raw_bucket_row() for bucket in bucket_values],
    }


def add_raw_bucket_to_streaming_aggregates(
    aggregate_buckets: dict[tuple[str, str, int], dict],
    raw_bucket: dict,
) -> None:
    """Merge one completed 5m bucket into bounded aggregate state."""
    for granularity, seconds in AGGREGATE_GRANULARITY_SECONDS:
        bucket_start = floor_bucket_start(raw_bucket['bucket_start'], seconds)
        key = (raw_bucket['source_id'], granularity, bucket_start)
        aggregate = aggregate_buckets.setdefault(
            key,
            new_streaming_aggregate_bucket(raw_bucket['source_id'], granularity, bucket_start),
        )
        aggregate['source_ipv4'].update(raw_bucket['source_ipv4'])
        aggregate['destination_ipv4'].update(raw_bucket['destination_ipv4'])
        aggregate['source_ipv6'].update(raw_bucket['source_ipv6'])
        aggregate['destination_ipv6'].update(raw_bucket['destination_ipv6'])
        aggregate['protocols_ipv4'].update(raw_bucket['protocols_ipv4'])
        aggregate['protocols_ipv6'].update(raw_bucket['protocols_ipv6'])
        aggregate['maad_source_ipv4'].update(raw_bucket['maad_source_ipv4'])
        aggregate['maad_destination_ipv4'].update(raw_bucket['maad_destination_ipv4'])
        for row in raw_bucket['netflow_rows']:
            add_netflow_row_to_aggregate(aggregate, row)


def new_streaming_aggregate_bucket(source_id: str, granularity: str, bucket_start: int) -> dict:
    """Create an aggregate bucket backed by mutable sets."""
    bucket_seconds = dict(AGGREGATE_GRANULARITY_SECONDS)[granularity]
    return {
        'source_id': source_id,
        'granularity': granularity,
        'bucket_start': bucket_start,
        'bucket_end': next_bucket_start(bucket_start, bucket_seconds),
        'source_ipv4': set(),
        'destination_ipv4': set(),
        'source_ipv6': set(),
        'destination_ipv6': set(),
        'protocols_ipv4': set(),
        'protocols_ipv6': set(),
        'maad_source_ipv4': set(),
        'maad_destination_ipv4': set(),
        'netflow_by_version': {},
    }


def add_netflow_row_to_aggregate(bucket: dict, row: dict) -> None:
    """Sum one 5m netflow row into a coarser aggregate bucket."""
    ip_version = validate_ip_version(row['ip_version'])
    aggregate = bucket['netflow_by_version'].setdefault(
        ip_version,
        new_netflow_bucket_from_values(
            source_id=bucket['source_id'],
            bucket_start=bucket['bucket_start'],
            bucket_end=bucket['bucket_end'],
            ip_version=ip_version,
        ),
    )
    aggregate['granularity'] = bucket['granularity']
    for column in NETFLOW_METRIC_COLUMNS:
        aggregate[column] += row[column]


def flush_streaming_aggregate_buckets(
    conn: sqlite3.Connection,
    aggregate_buckets: dict[tuple[str, str, int], dict],
    keys: list[tuple[str, str, int]],
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
    run_maad: bool,
) -> None:
    """Write and discard aggregate buckets that cannot receive more rows."""
    if not keys:
        return
    buckets = [aggregate_buckets.pop(key) for key in sorted(keys)]
    netflow_rows = [
        row
        for bucket in buckets
        for row in build_streaming_aggregate_netflow_rows(bucket)
    ]
    ip_rows = [build_streaming_aggregate_ip_row(bucket) for bucket in buckets]
    protocol_rows = [build_streaming_aggregate_protocol_row(bucket) for bucket in buckets]
    maad_rows = (
        build_streaming_aggregate_maad_rows(buckets, maad_bin, maad_backend, maad_workers)
        if run_maad
        else []
    )
    with conn:
        insert_netflow_stats_aggregate_v2_rows(conn, netflow_rows)
        insert_ip_stats_v2_rows(conn, ip_rows)
        insert_protocol_stats_v2_rows(conn, protocol_rows)
        for rows in maad_rows:
            insert_maad_v2_rows(conn, rows)


def build_streaming_aggregate_netflow_rows(bucket: dict) -> list[dict]:
    """Build aggregate netflow_stats_aggregate_v2 rows from streaming state."""
    return [
        bucket['netflow_by_version'][ip_version]
        for ip_version in sorted(bucket['netflow_by_version'])
    ]


def build_streaming_aggregate_ip_row(bucket: dict) -> dict:
    """Build one aggregate ip_stats_v2 row from streaming state."""
    return {
        'source_id': bucket['source_id'],
        'granularity': bucket['granularity'],
        'bucket_start': bucket['bucket_start'],
        'bucket_end': bucket['bucket_end'],
        'sa_ipv4_count': len(bucket['source_ipv4']),
        'da_ipv4_count': len(bucket['destination_ipv4']),
        'sa_ipv6_count': len(bucket['source_ipv6']),
        'da_ipv6_count': len(bucket['destination_ipv6']),
    }


def build_streaming_aggregate_protocol_row(bucket: dict) -> dict:
    """Build one aggregate protocol_stats_v2 row from streaming state."""
    return {
        'source_id': bucket['source_id'],
        'granularity': bucket['granularity'],
        'bucket_start': bucket['bucket_start'],
        'bucket_end': bucket['bucket_end'],
        'unique_protocols_count_ipv4': len(bucket['protocols_ipv4']),
        'unique_protocols_count_ipv6': len(bucket['protocols_ipv6']),
        'protocols_list_ipv4': ','.join(sorted(bucket['protocols_ipv4'])),
        'protocols_list_ipv6': ','.join(sorted(bucket['protocols_ipv6'])),
    }


def build_streaming_aggregate_maad_rows(
    buckets: list[dict],
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
) -> list[dict[str, dict]]:
    """Build MAAD rows from already-unioned streaming aggregate buckets."""
    tasks = [
        {
            'maad_bin': str(maad_bin),
            'maad_backend': maad_backend,
            'source_id': bucket['source_id'],
            'granularity': bucket['granularity'],
            'bucket_start': bucket['bucket_start'],
            'bucket_end': bucket['bucket_end'],
            'source_addresses': sorted(bucket['maad_source_ipv4']),
            'destination_addresses': sorted(bucket['maad_destination_ipv4']),
        }
        for bucket in buckets
    ]
    return process_maad_tasks(tasks, maad_workers)


def iter_input_payloads(tasks: list[tuple[dict, str, str, int, bool]], max_workers: int) -> Iterable[dict]:
    """Yield worker payloads serially or through a process pool."""
    if max_workers > 1 and len(tasks) > 1:
        with Pool(processes=max_workers) as pool:
            yield from pool.imap_unordered(process_input_spec_worker, tasks, chunksize=1)
        return

    for task in tasks:
        yield process_input_spec_worker(task)


def process_input_spec_worker(task: tuple[dict, str, str, int, bool]) -> dict:
    """Worker entrypoint for processing one input spec without DB access."""
    spec, maad_bin, maad_backend, maad_workers, run_maad = task
    return build_input_payload(spec, maad_bin, maad_backend, maad_workers, run_maad)


def build_input_payload(
    spec: dict,
    maad_bin: str | Path,
    maad_backend: str = 'subprocess',
    maad_workers: int = 1,
    run_maad: bool = True,
) -> dict:
    """Build all DB insert payloads for one input spec."""
    input_kind = str(spec['input_kind'])
    input_locator = str(spec['path'])
    if input_kind == 'nfcapd':
        if spec.get('gap'):
            return build_nfcapd_gap_payload(
                input_locator=input_locator,
                source_id=str(spec['source_id']),
                bucket_start=int(spec['bucket_start']),
                run_maad=run_maad,
            )
        nfcapd_payload = build_nfcapd_bucket_payload(input_locator, str(spec['source_id']))
        return {
            'processed_buckets': [nfcapd_payload['processed_bucket']],
            'netflow_rows': nfcapd_payload['netflow_rows'],
            'ip_rows': [nfcapd_payload['ip_row']],
            'protocol_rows': [nfcapd_payload['protocol_row']],
            'maad_rows': [
                build_maad_rows_for_raw_bucket(
                    nfcapd_payload['raw_bucket'],
                    '5m',
                    maad_bin,
                    maad_backend,
                )
            ]
            if run_maad
            else [],
            'raw_buckets': [nfcapd_payload['raw_bucket']],
        }

    buckets = accumulate_input_buckets(iter_input_rows(spec))
    bucket_values = [buckets[key] for key in sorted(buckets)]

    return {
        'processed_buckets': [
            {
                'input_kind': input_kind,
                'input_locator': input_locator,
                'source_id': source_id,
                'bucket_start': bucket_start,
                'bucket_end': bucket_end,
            }
            for source_id, bucket_start, bucket_end in sorted(buckets)
        ],
        'netflow_rows': [row for bucket in bucket_values for row in bucket.netflow_rows()],
        'ip_rows': [bucket.ip_row() for bucket in bucket_values],
        'protocol_rows': [bucket.protocol_row() for bucket in bucket_values],
        'maad_rows': build_maad_payload_rows(buckets, maad_bin, maad_backend, maad_workers) if run_maad else [],
        'raw_buckets': [bucket.raw_bucket_row() for bucket in bucket_values],
    }


def write_input_payload(conn: sqlite3.Connection, payload: dict, *, mark_processed: bool = True) -> None:
    """Persist a worker payload. SQLite writes remain in the parent process."""
    processed_buckets = payload['processed_buckets']
    if not processed_buckets:
        return

    with conn:
        for bucket in processed_buckets:
            upsert_input_bucket(conn, **bucket)

        insert_netflow_stats_v2_rows(conn, payload['netflow_rows'])
        insert_ip_stats_v2_rows(conn, payload['ip_rows'])
        insert_protocol_stats_v2_rows(conn, payload['protocol_rows'])

        for rows in payload['maad_rows']:
            insert_maad_v2_rows(conn, rows)

        if mark_processed:
            mark_processed_buckets(conn, processed_buckets)


def mark_processed_buckets(conn: sqlite3.Connection, processed_buckets: list[dict]) -> None:
    """Mark input buckets processed after all v2 outputs are written."""
    for bucket in processed_buckets:
        mark_input_bucket_status(
            conn,
            input_kind=bucket['input_kind'],
            input_locator=bucket['input_locator'],
            source_id=bucket['source_id'],
            bucket_start=bucket['bucket_start'],
            status='processed',
        )


def write_aggregate_rows(
    conn: sqlite3.Connection,
    raw_buckets: list[dict],
    maad_bin: str | Path,
    max_workers: int,
    *,
    maad_backend: str = 'subprocess',
    maad_workers: int = 1,
    run_maad: bool = True,
) -> None:
    """Write 30m, 1h, and 1d aggregate rows from raw bucket sets."""
    if not raw_buckets:
        return
    netflow_rows = build_aggregate_netflow_rows(raw_buckets)
    ip_rows = build_aggregate_ip_rows(raw_buckets)
    protocol_rows = build_aggregate_protocol_rows(raw_buckets)
    maad_rows = (
        build_aggregate_maad_rows(raw_buckets, maad_bin, maad_backend, maad_workers, max_workers)
        if run_maad
        else []
    )
    with conn:
        insert_netflow_stats_aggregate_v2_rows(conn, netflow_rows)
        insert_ip_stats_v2_rows(conn, ip_rows)
        insert_protocol_stats_v2_rows(conn, protocol_rows)
        for rows in maad_rows:
            insert_maad_v2_rows(conn, rows)


def build_aggregate_netflow_rows(raw_buckets: list[dict]) -> list[dict]:
    """Build netflow aggregate rows from raw 5m netflow stats."""
    buckets: dict[tuple[str, str, int, int], dict] = {}

    for raw in raw_buckets:
        for granularity, seconds in AGGREGATE_GRANULARITY_SECONDS:
            bucket_start = floor_bucket_start(raw['bucket_start'], seconds)
            bucket_end = next_bucket_start(bucket_start, seconds)
            for raw_row in raw['netflow_rows']:
                ip_version = validate_ip_version(raw_row['ip_version'])
                key = (raw['source_id'], granularity, bucket_start, ip_version)
                bucket = buckets.setdefault(
                    key,
                    new_netflow_bucket_from_values(
                        source_id=raw['source_id'],
                        bucket_start=bucket_start,
                        bucket_end=bucket_end,
                        ip_version=ip_version,
                    ),
                )
                bucket['granularity'] = granularity
                for column in NETFLOW_METRIC_COLUMNS:
                    bucket[column] += raw_row[column]

    return [buckets[key] for key in sorted(buckets)]


def build_aggregate_ip_rows(raw_buckets: list[dict]) -> list[dict]:
    """Build v1-parity IP aggregate rows from raw 5m bucket sets."""
    buckets = defaultdict(
        lambda: {
            'source_ipv4': set(),
            'destination_ipv4': set(),
            'source_ipv6': set(),
            'destination_ipv6': set(),
        }
    )

    for raw in raw_buckets:
        for granularity, seconds in AGGREGATE_GRANULARITY_SECONDS:
            bucket_start = floor_bucket_start(raw['bucket_start'], seconds)
            bucket = buckets[(raw['source_id'], granularity, bucket_start)]
            bucket['source_ipv4'].update(raw['source_ipv4'])
            bucket['destination_ipv4'].update(raw['destination_ipv4'])
            bucket['source_ipv6'].update(raw['source_ipv6'])
            bucket['destination_ipv6'].update(raw['destination_ipv6'])

    rows = []
    for (source_id, granularity, bucket_start), bucket in sorted(buckets.items()):
        bucket_seconds = dict(AGGREGATE_GRANULARITY_SECONDS)[granularity]
        rows.append(
            {
                'source_id': source_id,
                'granularity': granularity,
                'bucket_start': bucket_start,
                'bucket_end': next_bucket_start(bucket_start, bucket_seconds),
                'sa_ipv4_count': len(bucket['source_ipv4']),
                'da_ipv4_count': len(bucket['destination_ipv4']),
                'sa_ipv6_count': len(bucket['source_ipv6']),
                'da_ipv6_count': len(bucket['destination_ipv6']),
            }
        )
    return rows


def build_aggregate_protocol_rows(raw_buckets: list[dict]) -> list[dict]:
    """Build v1-parity protocol aggregate rows from raw 5m bucket sets."""
    buckets = defaultdict(lambda: {'ipv4': set(), 'ipv6': set()})

    for raw in raw_buckets:
        for granularity, seconds in AGGREGATE_GRANULARITY_SECONDS:
            bucket_start = floor_bucket_start(raw['bucket_start'], seconds)
            bucket = buckets[(raw['source_id'], granularity, bucket_start)]
            bucket['ipv4'].update(raw['protocols_ipv4'])
            bucket['ipv6'].update(raw['protocols_ipv6'])

    rows = []
    for (source_id, granularity, bucket_start), bucket in sorted(buckets.items()):
        bucket_seconds = dict(AGGREGATE_GRANULARITY_SECONDS)[granularity]
        rows.append(
            {
                'source_id': source_id,
                'granularity': granularity,
                'bucket_start': bucket_start,
                'bucket_end': next_bucket_start(bucket_start, bucket_seconds),
                'unique_protocols_count_ipv4': len(bucket['ipv4']),
                'unique_protocols_count_ipv6': len(bucket['ipv6']),
                'protocols_list_ipv4': ','.join(sorted(bucket['ipv4'])),
                'protocols_list_ipv6': ','.join(sorted(bucket['ipv6'])),
            }
        )
    return rows


def build_aggregate_maad_rows(
    raw_buckets: list[dict],
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
    max_workers: int,
) -> list[dict[str, dict]]:
    """Build aggregate MAAD rows from unioned IPv4 address sets."""
    tasks = build_aggregate_maad_tasks(raw_buckets, maad_bin, maad_backend)
    aggregate_workers = aggregate_maad_worker_count(max(max_workers, maad_workers))
    if tasks:
        print(f'[pipeline_v2] Aggregate MAAD: {len(tasks)} tasks with {aggregate_workers} workers')
    if aggregate_workers > 1 and len(tasks) > 1:
        return process_maad_tasks(tasks, aggregate_workers)
    return process_maad_tasks(tasks, 1)


def build_aggregate_maad_tasks(
    raw_buckets: list[dict],
    maad_bin: str | Path,
    maad_backend: str,
) -> list[dict]:
    """Build serializable MAAD tasks for aggregate buckets."""
    buckets = defaultdict(lambda: {'source': set(), 'destination': set()})

    for raw in raw_buckets:
        for granularity, seconds in AGGREGATE_GRANULARITY_SECONDS:
            bucket_start = floor_bucket_start(raw['bucket_start'], seconds)
            bucket = buckets[(raw['source_id'], granularity, bucket_start)]
            bucket['source'].update(raw['maad_source_ipv4'])
            bucket['destination'].update(raw['maad_destination_ipv4'])

    tasks = []
    for (source_id, granularity, bucket_start), bucket in sorted(buckets.items()):
        bucket_seconds = dict(AGGREGATE_GRANULARITY_SECONDS)[granularity]
        tasks.append(
            {
                'maad_bin': str(maad_bin),
                'maad_backend': maad_backend,
                'source_id': source_id,
                'granularity': granularity,
                'bucket_start': bucket_start,
                'bucket_end': next_bucket_start(bucket_start, bucket_seconds),
                'source_addresses': sorted(bucket['source']),
                'destination_addresses': sorted(bucket['destination']),
                'log_progress': True,
            }
        )
    return tasks


def process_maad_row_task(task: dict) -> dict[str, dict]:
    """Run MAAD for one aggregate task."""
    timeout_seconds = maad_timeout_seconds(str(task['granularity']))
    bucket_label = datetime.fromtimestamp(task['bucket_start'], PIPELINE_TIMEZONE).isoformat()
    source_count = len(task['source_addresses'])
    destination_count = len(task['destination_addresses'])
    if task.get('log_progress'):
        print(
            '[pipeline_v2] Aggregate MAAD task '
            f"{task['source_id']} {task['granularity']} {bucket_label} "
            f'sa={source_count} da={destination_count} timeout={timeout_seconds}s'
        )

    try:
        source_result = run_maad_for_addresses(
            task['maad_bin'],
            set(task['source_addresses']),
            maad_backend=str(task.get('maad_backend', 'subprocess')),
            timeout_seconds=timeout_seconds,
        )
    except MaadTimeoutError as error:
        raise MaadTimeoutError(
            'MAAD timed out for '
            f"{task['source_id']} {task['granularity']} {bucket_label} "
            f'source addresses={source_count} timeout={timeout_seconds}s'
        ) from error
    try:
        destination_result = run_maad_for_addresses(
            task['maad_bin'],
            set(task['destination_addresses']),
            maad_backend=str(task.get('maad_backend', 'subprocess')),
            timeout_seconds=timeout_seconds,
        )
    except MaadTimeoutError as error:
        raise MaadTimeoutError(
            'MAAD timed out for '
            f"{task['source_id']} {task['granularity']} {bucket_label} "
            f'destination addresses={destination_count} timeout={timeout_seconds}s'
        ) from error
    return build_maad_v2_rows(
        source_id=task['source_id'],
        granularity=task['granularity'],
        bucket_start=task['bucket_start'],
        bucket_end=task['bucket_end'],
        ip_version=4,
        source_result=source_result,
        destination_result=destination_result,
    )


def build_maad_rows_for_raw_bucket(
    raw_bucket: dict,
    granularity: str,
    maad_bin: str | Path,
    maad_backend: str = 'subprocess',
) -> dict[str, dict]:
    """Run MAAD for one raw bucket payload."""
    bucket_seconds = 300 if granularity == '5m' else dict(AGGREGATE_GRANULARITY_SECONDS)[granularity]
    return process_maad_row_task(
        {
            'maad_bin': str(maad_bin),
            'maad_backend': maad_backend,
            'source_id': raw_bucket['source_id'],
            'granularity': granularity,
            'bucket_start': raw_bucket['bucket_start'],
            'bucket_end': next_bucket_start(raw_bucket['bucket_start'], bucket_seconds),
            'source_addresses': raw_bucket['maad_source_ipv4'],
            'destination_addresses': raw_bucket['maad_destination_ipv4'],
            'log_progress': False,
        }
    )


def maad_timeout_seconds(granularity: str) -> int:
    """Return the MAAD timeout for the provided bucket granularity."""
    try:
        return MAAD_TIMEOUT_SECONDS_BY_GRANULARITY[granularity]
    except KeyError as error:
        raise ValueError(f'Unsupported MAAD granularity: {granularity}') from error


def aggregate_maad_worker_count(max_workers: int) -> int:
    """Bound aggregate MAAD concurrency to reduce timeout risk from contention."""
    return max(1, min(max_workers, DEFAULT_AGGREGATE_MAAD_MAX_WORKERS))


def process_maad_tasks(tasks: list[dict], maad_workers: int) -> list[dict[str, dict]]:
    """Process MAAD tasks serially or with a bounded worker pool."""
    if maad_workers > 1 and len(tasks) > 1:
        with Pool(processes=maad_workers) as pool:
            return list(pool.imap_unordered(process_maad_row_task, tasks, chunksize=1))
    return [process_maad_row_task(task) for task in tasks]


def run_maad_for_addresses(
    maad_bin: str | Path,
    addresses: set[str],
    *,
    maad_backend: str,
    timeout_seconds: int,
) -> MaadJsonResult:
    """Run MAAD through the configured backend."""
    if maad_backend == 'python':
        return compute_maad_json(addresses)
    if maad_backend == 'subprocess':
        return run_maad_json(maad_bin, addresses, timeout_seconds=timeout_seconds)
    raise ValueError(f'Unsupported MAAD backend: {maad_backend}')


def floor_bucket_start(bucket_start: int, bucket_seconds: int) -> int:
    """Floor a 5m bucket start to a local-time aggregate bucket."""
    timestamp = datetime.fromtimestamp(bucket_start, PIPELINE_TIMEZONE)
    if bucket_seconds == 1800:
        floored = timestamp.replace(
            minute=(timestamp.minute // 30) * 30,
            second=0,
            microsecond=0,
        )
        return int(floored.timestamp())
    if bucket_seconds == 3600:
        floored = timestamp.replace(minute=0, second=0, microsecond=0)
        return int(floored.timestamp())
    if bucket_seconds == 86400:
        floored = timestamp.replace(hour=0, minute=0, second=0, microsecond=0)
        return int(floored.timestamp())
    raise ValueError(f'Unsupported aggregate bucket size: {bucket_seconds}')


def next_bucket_start(bucket_start: int, bucket_seconds: int) -> int:
    """Return the next local-time bucket boundary after bucket_start."""
    timestamp = datetime.fromtimestamp(bucket_start, PIPELINE_TIMEZONE)
    if bucket_seconds == 300:
        return int((timestamp + timedelta(minutes=5)).timestamp())
    if bucket_seconds == 1800:
        boundary = timestamp.replace(
            minute=(timestamp.minute // 30) * 30,
            second=0,
            microsecond=0,
        )
        return int((boundary + timedelta(minutes=30)).timestamp())
    if bucket_seconds == 3600:
        boundary = timestamp.replace(minute=0, second=0, microsecond=0)
        return int((boundary + timedelta(hours=1)).timestamp())
    if bucket_seconds == 86400:
        boundary = timestamp.replace(hour=0, minute=0, second=0, microsecond=0)
        return int((boundary + timedelta(days=1)).timestamp())
    raise ValueError(f'Unsupported bucket size: {bucket_seconds}')


def iter_input_rows(spec: dict) -> Iterable[NormalizedRow]:
    """Yield normalized rows for one explicit input spec."""
    input_kind = str(spec['input_kind'])
    input_path = str(spec['path'])
    if input_kind == 'csv':
        mapping_path = str(spec['mapping_path'])
        yield from iter_csv_rows(input_path, mapping_path)
        return
    if input_kind == 'nfcapd':
        source_id = str(spec['source_id'])
        yield from iter_nfdump_rows(input_path, source_id)
        return
    raise ValueError(f'Unsupported input_kind: {input_kind}')


def build_nfcapd_gap_payload(
    input_locator: str,
    source_id: str,
    bucket_start: int,
    *,
    run_maad: bool = True,
) -> dict:
    """Build zero-valued v2 rows for one bounded missing nfcapd bucket."""
    bucket_end = bucket_start + FIVE_MINUTE_SECONDS
    netflow_rows = [
        new_netflow_bucket_from_values(
            source_id=source_id,
            bucket_start=bucket_start,
            bucket_end=bucket_end,
            ip_version=ip_version,
        )
        for ip_version in (4, 6)
    ]
    raw_bucket = {
        'source_id': source_id,
        'bucket_start': bucket_start,
        'source_ipv4': [],
        'destination_ipv4': [],
        'source_ipv6': [],
        'destination_ipv6': [],
        'protocols_ipv4': [],
        'protocols_ipv6': [],
        'maad_source_ipv4': [],
        'maad_destination_ipv4': [],
        'netflow_rows': [dict(row) for row in netflow_rows],
    }
    return {
        'processed_buckets': [
            {
                'input_kind': 'nfcapd',
                'input_locator': input_locator,
                'source_id': source_id,
                'bucket_start': bucket_start,
                'bucket_end': bucket_end,
            }
        ],
        'netflow_rows': netflow_rows,
        'ip_rows': [
            {
                'source_id': source_id,
                'granularity': '5m',
                'bucket_start': bucket_start,
                'bucket_end': bucket_end,
                'sa_ipv4_count': 0,
                'da_ipv4_count': 0,
                'sa_ipv6_count': 0,
                'da_ipv6_count': 0,
            }
        ],
        'protocol_rows': [
            {
                'source_id': source_id,
                'granularity': '5m',
                'bucket_start': bucket_start,
                'bucket_end': bucket_end,
                'unique_protocols_count_ipv4': 0,
                'unique_protocols_count_ipv6': 0,
                'protocols_list_ipv4': '',
                'protocols_list_ipv6': '',
            }
        ],
        'maad_rows': [build_maad_rows_for_raw_bucket(raw_bucket, '5m', '', maad_backend='python')]
        if run_maad
        else [],
        'raw_buckets': [raw_bucket],
    }


def accumulate_input_buckets(rows: Iterable[NormalizedRow]) -> dict[tuple[str, int, int], BucketAccumulator]:
    """Accumulate normalized rows by source and 5-minute bucket."""
    buckets: dict[tuple[str, int, int], BucketAccumulator] = {}
    for row in rows:
        add_row_to_bucket(buckets, row)
    return buckets


def add_row_to_bucket(
    buckets: dict[tuple[str, int, int], BucketAccumulator],
    row: NormalizedRow,
) -> None:
    """Accumulate one normalized row into a bucket map."""
    key = (row.source_id, row.bucket_start, row.bucket_end)
    bucket = buckets.setdefault(
        key,
        BucketAccumulator(
            source_id=row.source_id,
            bucket_start=row.bucket_start,
            bucket_end=row.bucket_end,
        ),
    )
    bucket.add(row)


def new_netflow_bucket(row: NormalizedRow) -> dict:
    """Create an empty netflow_stats_v2 row accumulator."""
    return new_netflow_bucket_from_values(
        source_id=row.source_id,
        bucket_start=row.bucket_start,
        bucket_end=row.bucket_end,
        ip_version=row.ip_version,
    )


def new_netflow_bucket_from_values(
    *,
    source_id: str,
    bucket_start: int,
    bucket_end: int,
    ip_version: int,
) -> dict:
    """Create an empty netflow_stats_v2 row accumulator from primitive values."""
    return {
        'source_id': source_id,
        'bucket_start': bucket_start,
        'bucket_end': bucket_end,
        'ip_version': ip_version,
        'flows': 0,
        'flows_tcp': 0,
        'flows_udp': 0,
        'flows_icmp': 0,
        'flows_other': 0,
        'packets': 0,
        'packets_tcp': 0,
        'packets_udp': 0,
        'packets_icmp': 0,
        'packets_other': 0,
        'bytes': 0,
        'bytes_tcp': 0,
        'bytes_udp': 0,
        'bytes_icmp': 0,
        'bytes_other': 0,
    }


def build_maad_payload_rows(
    buckets: dict[tuple[str, int, int], BucketAccumulator],
    maad_bin: str | Path,
    maad_backend: str = 'subprocess',
    maad_workers: int = 1,
) -> list[dict[str, dict]]:
    """Run MAAD for each v2 bucket and return structure/spectrum/dimension rows."""
    tasks = [
        {
            'maad_bin': str(maad_bin),
            'maad_backend': maad_backend,
            'source_id': bucket.source_id,
            'granularity': '5m',
            'bucket_start': bucket.bucket_start,
            'bucket_end': bucket.bucket_end,
            'source_addresses': sorted(bucket.maad_source_ipv4),
            'destination_addresses': sorted(bucket.maad_destination_ipv4),
        }
        for bucket in buckets.values()
    ]
    return process_maad_tasks(tasks, maad_workers)


def iter_nfdump_rows(path: str, source_id: str) -> Iterable[NormalizedRow]:
    """Yield normalized rows from an nfcapd file via nfdump CSV output."""
    for ip_version in (4, 6):
        command = build_nfdump_csv_command(path, ip_version)
        result = subprocess.run(command, capture_output=True, text=True, timeout=300)
        if result.returncode != 0:
            raise RuntimeError(
                f"nfdump failed for {path} family {ip_version}: {result.stderr.strip()}"
            )
        for values in csv.reader(result.stdout.splitlines()):
            if not values:
                continue
            if looks_like_nfdump_header(values):
                continue
            yield normalize_nfdump_csv_values(values, source_id)


def looks_like_nfdump_header(values: list[str]) -> bool:
    """Return true when the csv row looks like a textual header."""
    first_value = values[0].strip().lower()
    if first_value in NFDUMP_HEADER_FIRST_VALUES:
        return True
    try:
        float(first_value)
        return False
    except ValueError:
        LOGGER.warning('Malformed nfdump CSV row with non-numeric timestamp: %s', values)
        return False


def main() -> None:
    """Run the minimal pipeline v2 entrypoint."""
    parser = argparse.ArgumentParser(description='Pipeline v2 processor')
    parser.add_argument('--config', help='Path to the pipeline_v2 json config.')
    parser.add_argument('--dataset', help='Dataset id from datasets.json for canonical nfcapd tree input.')
    parser.add_argument('--start-date', help='Start date for --dataset, inclusive, YYYY-MM-DD.')
    parser.add_argument('--end-date', help='End date for --dataset, inclusive, YYYY-MM-DD. Defaults to latest nfcapd day.')
    parser.add_argument('--database-path', help='Override v2 SQLite output path.')
    parser.add_argument('--maad-bin', default=str(DEFAULT_MAAD_BIN), help='Path to MAAD binary.')
    parser.add_argument('--max-workers', type=int, default=DEFAULT_MAX_WORKERS, help='Worker process count.')
    args = parser.parse_args()

    if args.config:
        config = load_pipeline_v2_config(args.config)
        if args.database_path:
            config['database_path'] = args.database_path
    else:
        if not args.dataset or not args.start_date:
            parser.error('--config or both --dataset and --start-date is required')
        config = build_dataset_tree_config(
            dataset_id=args.dataset,
            start_date=args.start_date,
            end_date=args.end_date,
            database_path=args.database_path,
            maad_bin=args.maad_bin,
            max_workers=args.max_workers,
        )

    db_path = Path(config['database_path'])
    db_path.parent.mkdir(parents=True, exist_ok=True)

    with sqlite3.connect(db_path) as conn:
        process_pipeline_v2_config(conn, config)


if __name__ == '__main__':
    main()
