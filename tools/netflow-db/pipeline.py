#!/usr/bin/env python3
"""
Pipeline entrypoint.

Processes explicit csv and nfcapd inputs into canonical netflow tables.
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
from dataclasses import dataclass
from datetime import datetime, timedelta
from functools import lru_cache
from multiprocessing import Pool
from pathlib import Path
from typing import Iterable
from zoneinfo import ZoneInfo

from csv_ingest import (
    TIMESTAMP_KEYS,
    CsvSourceConfig,
    CsvSourceConfigError,
    load_csv_source_config,
    parse_timestamp,
)
from csv_inputs import (
    build_field_indexes,
    discover_csv_specs,
    iter_csv_rows,
    iter_headerless_csv_values,
    is_tar_archive,
)
from datasets_metadata import init_datasets_table, upsert_dataset_metadata
from maad import (
    MaadTimeoutError,
    MaadJsonResult,
    compute_maad_json,
    run_maad_json,
)
from nfdump_stats import (
    build_nfcapd_bucket_payload,
    is_nfcapd_bucket_filename,
    parse_nfcapd_bucket_start,
)
from normalized_rows import NormalizedRow, build_nfdump_csv_command, normalize_nfdump_csv_values
from normalized_rows import infer_ip_version
from processed_inputs import (
    init_processed_inputs_table,
    mark_input_bucket_status,
    upsert_input_bucket,
)
from statistical_bucket import (
    BucketKey,
    CanonicalBucket,
    FlowFact,
    StatisticalBucket,
    ZERO_FILL_VISIBILITY_PAIRS,
)
from stats import (
    build_address_structure_stats_rows,
    canonical_bucket_rows,
    init_stats_tables,
    insert_address_count_stats_rows,
    insert_address_structure_stats_rows,
    insert_protocol_stats_rows,
    insert_traffic_stats_rows,
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
CSV_ARROW_REQUIRED_COLUMN_KEYS = (
    'time_end',
    'src_ip',
    'dst_ip',
    'protocol',
    'packets',
    'bytes',
    'src_tos',
)


@dataclass(frozen=True)
class SourceDefinition:
    """Logical source backed by one or more physical nfcapd member directories."""

    source_id: str
    members: tuple[str, ...]


def load_pipeline_config(path: str | Path) -> dict:
    """Load the minimal pipeline config file."""
    with open(path, 'r', encoding='utf-8') as handle:
        payload = json.load(handle)
    if not isinstance(payload, dict):
        raise ValueError('pipeline config must be a json object')
    inputs = payload.get('inputs')
    if not isinstance(inputs, list):
        raise ValueError("pipeline config must include an 'inputs' list")
    return payload


def apply_cli_config_overrides(
    config: dict,
    *,
    database_path: str | None = None,
    maad_bin: str | None = None,
    max_workers: int | None = None,
    force: bool = False,
) -> dict:
    """Apply explicit CLI overrides to a loaded pipeline config."""
    if database_path:
        config['database_path'] = database_path
    if maad_bin is not None:
        config['maad_bin'] = maad_bin
    if max_workers is not None:
        config['max_workers'] = validate_max_workers(max_workers)
    if force:
        nfcapd_tree_inputs = [
            spec for spec in config.get('inputs', []) if spec.get('input_kind') == 'nfcapd_tree'
        ]
        if len(nfcapd_tree_inputs) != 1:
            raise ValueError('--force in --config mode requires exactly one nfcapd_tree input')
        nfcapd_tree_inputs[0]['force'] = True
    return config


def validate_max_workers(max_workers: int) -> int:
    """Return a validated worker count."""
    if max_workers < 1:
        raise ValueError('max_workers must be at least 1')
    return max_workers


def process_pipeline_config(conn: sqlite3.Connection, config: dict) -> None:
    """Process a config, including canonical nfcapd tree inputs."""
    init_datasets_table(conn)
    for dataset in config.get('datasets', []):
        upsert_dataset_metadata(conn, dataset)

    maad_bin = config.get('maad_bin', DEFAULT_MAAD_BIN)
    maad_backend = str(config.get('maad_backend', 'subprocess'))
    maad_workers = int(config.get('maad_workers', 1))
    max_workers = validate_max_workers(int(config.get('max_workers', DEFAULT_MAX_WORKERS)))
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
    sources = normalize_nfcapd_sources(spec, root_path)
    member_ids = sorted({member for source in sources for member in source.members})
    start_date = parse_config_date(str(spec['start_date']))
    explicit_end_date = bool(spec.get('end_date'))
    end_date = (
        parse_config_date(str(spec['end_date']))
        if explicit_end_date
        else discover_latest_nfcapd_tree_day(root_path, member_ids)
    )
    zero_fill_gaps = bool(spec.get('zero_fill_gaps', True))
    start_bucket = parse_optional_config_time(spec.get('start_time'))
    end_bucket = parse_optional_config_time(spec.get('end_time'))
    validate_aggregate_safe_time_window(
        start_bucket,
        end_bucket,
        start_date=start_date,
        end_date=end_date,
    )
    source_bounds = (
        build_nfcapd_zero_fill_source_bounds(
            member_ids,
            discover_nfcapd_source_bounds(root_path, member_ids),
            start_date=start_date,
            end_date=end_date,
            start_bucket=start_bucket,
            end_bucket=end_bucket,
            extend_to_requested_window=explicit_end_date,
        )
        if zero_fill_gaps
        else {}
    )
    force = bool(spec.get('force', False))

    for day in iter_days(start_date, end_date):
        member_specs = discover_nfcapd_tree_specs(
            root_path,
            member_ids,
            day,
            source_bounds=source_bounds,
        )
        if start_bucket is not None or end_bucket is not None:
            member_specs = filter_input_specs_by_bucket_window(
                member_specs,
                start_bucket=start_bucket,
                end_bucket=end_bucket,
            )
        if not member_specs:
            LOGGER.info('No nfcapd files found for %s', day.strftime('%Y-%m-%d'))
            continue
        jobs = build_nfcapd_logical_bucket_jobs(conn, sources, member_specs, force=force)
        if not jobs:
            print(f"[pipeline] Skip {day.strftime('%Y-%m-%d')}: {len(member_specs)} already processed")
            continue
        print(
            f"[pipeline] Processing {day.strftime('%Y-%m-%d')}: "
            f"{len(jobs)} logical nfcapd buckets from {len(member_specs)} member inputs"
        )
        process_nfcapd_logical_bucket_jobs(
            conn,
            jobs,
            maad_bin=maad_bin,
            maad_backend=maad_backend,
            maad_workers=maad_workers,
            max_workers=max_workers,
            run_maad=run_maad,
        )
        print(
            f"[pipeline] Complete {day.strftime('%Y-%m-%d')}: "
            f"{len(jobs)} logical nfcapd buckets"
        )


def normalize_nfcapd_sources(spec: dict, root_path: Path) -> list[SourceDefinition]:
    """Return authoritative logical nfcapd sources from config or discovered members."""
    if 'sources' in spec and 'source_ids' in spec:
        raise ValueError("nfcapd_tree input cannot define both 'sources' and 'source_ids'")

    raw_sources = spec.get('sources')
    if raw_sources is None:
        raw_source_ids = spec.get('source_ids')
        if raw_source_ids is None:
            raw_source_ids = discover_physical_member_ids(root_path)
        raw_sources = [
            {'source_id': str(source_id), 'members': [str(source_id)]}
            for source_id in raw_source_ids
        ]

    if not isinstance(raw_sources, list) or not raw_sources:
        raise ValueError("nfcapd_tree input must define a non-empty 'sources' list")

    sources = []
    seen_source_ids = set()
    for raw_source in raw_sources:
        if not isinstance(raw_source, dict):
            raise ValueError(f'Invalid source definition: {raw_source!r}')
        source_id = str(raw_source.get('source_id', '')).strip()
        raw_members = raw_source.get('members')
        if not source_id:
            raise ValueError('source_id is required for every nfcapd source')
        if source_id in seen_source_ids:
            raise ValueError(f"Duplicate source_id '{source_id}'")
        if not isinstance(raw_members, list) or not raw_members:
            raise ValueError(f"Source '{source_id}' must define a non-empty members list")
        members = tuple(str(member).strip() for member in raw_members if str(member).strip())
        if len(members) != len(raw_members) or not members:
            raise ValueError(f"Source '{source_id}' has an empty member id")
        if len(set(members)) != len(members):
            raise ValueError(f"Source '{source_id}' contains duplicate members")
        missing_members = [member for member in members if not (root_path / member).is_dir()]
        if missing_members:
            raise ValueError(
                f"Source '{source_id}' references missing member directories: {', '.join(missing_members)}"
            )
        seen_source_ids.add(source_id)
        sources.append(SourceDefinition(source_id=source_id, members=members))
    return sources


def discover_physical_member_ids(root_path: Path) -> list[str]:
    """Discover physical nfcapd member directories under a canonical root."""
    if not root_path.exists():
        return []
    return sorted(entry.name for entry in root_path.iterdir() if entry.is_dir())


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
    print(f"[pipeline] Processing {len(input_specs)} CSV inputs")
    process_input_specs(
        conn,
        input_specs,
        maad_bin=maad_bin,
        maad_backend=maad_backend,
        maad_workers=maad_workers,
        max_workers=max_workers,
        run_maad=run_maad,
    )
    print(f"[pipeline] Complete {len(input_specs)} CSV inputs")


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
            elif first_bucket <= bucket_start <= last_bucket:
                specs.append(build_nfcapd_gap_spec(root, source_id, bucket_start))
    return specs


def build_nfcapd_zero_fill_source_bounds(
    source_ids: list[str],
    discovered_bounds: dict[str, tuple[int, int]],
    *,
    start_date: datetime,
    end_date: datetime,
    start_bucket: int | None,
    end_bucket: int | None,
    extend_to_requested_window: bool,
) -> dict[str, tuple[int, int]]:
    """Return per-source bounds used to materialize missing nfcapd buckets."""
    if not extend_to_requested_window:
        return discovered_bounds

    requested_first, requested_last = nfcapd_requested_bucket_bounds(
        start_date,
        end_date,
        start_bucket=start_bucket,
        end_bucket=end_bucket,
    )
    return {
        source_id: extend_bucket_bounds(
            discovered_bounds.get(source_id),
            requested_first=requested_first,
            requested_last=requested_last,
        )
        for source_id in source_ids
    }


def nfcapd_requested_bucket_bounds(
    start_date: datetime,
    end_date: datetime,
    *,
    start_bucket: int | None,
    end_bucket: int | None,
) -> tuple[int, int]:
    """Return inclusive local 5-minute bucket bounds for the requested tree window."""
    first_bucket = next(iter(iter_local_day_bucket_starts(start_date)))
    last_bucket = max(iter_local_day_bucket_starts(end_date))
    if start_bucket is not None:
        first_bucket = max(first_bucket, start_bucket)
    if end_bucket is not None:
        last_bucket = min(last_bucket, end_bucket - FIVE_MINUTE_SECONDS)
    return first_bucket, last_bucket


def extend_bucket_bounds(
    bounds: tuple[int, int] | None,
    *,
    requested_first: int,
    requested_last: int,
) -> tuple[int, int]:
    """Extend discovered bounds to include the configured zero-fill window."""
    if bounds is None:
        return requested_first, requested_last
    first_bucket, last_bucket = bounds
    return min(first_bucket, requested_first), max(last_bucket, requested_last)


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


def build_nfcapd_logical_bucket_jobs(
    conn: sqlite3.Connection,
    sources: list[SourceDefinition],
    member_specs: list[dict],
    *,
    force: bool = False,
) -> list[dict]:
    """Build unprocessed logical source buckets from physical member specs."""
    specs_by_member_bucket: dict[tuple[str, int], list[dict]] = defaultdict(list)
    for spec in member_specs:
        member_id = str(spec['source_id'])
        bucket_start = int(spec.get('bucket_start') or parse_nfcapd_bucket_start(str(spec['path'])))
        specs_by_member_bucket[(member_id, bucket_start)].append(spec)

    jobs = []
    needs_processing = False
    for source in sources:
        for bucket_start in source_candidate_bucket_starts(source, specs_by_member_bucket):
            present_specs, missing_members = logical_source_member_specs(
                source,
                bucket_start,
                specs_by_member_bucket,
            )

            locators = (
                [str(spec['path']) for spec in present_specs]
                if present_specs
                else [logical_nfcapd_gap_locator(source.source_id, bucket_start)]
            )
            if force or not nfcapd_logical_bucket_processed(conn, source.source_id, bucket_start, locators):
                needs_processing = True
            jobs.append(
                {
                    'source_id': source.source_id,
                    'bucket_start': bucket_start,
                    'bucket_end': bucket_start + FIVE_MINUTE_SECONDS,
                    'member_specs': present_specs,
                    'missing_members': missing_members,
                }
            )
            if missing_members:
                bucket_label = datetime.fromtimestamp(bucket_start, PIPELINE_TIMEZONE).isoformat()
                print(
                    f"[pipeline] Partial source {source.source_id} {bucket_label}: "
                    f"missing members {', '.join(missing_members)}"
                )
    return jobs if needs_processing else []


def source_candidate_bucket_starts(
    source: SourceDefinition,
    specs_by_member_bucket: dict[tuple[str, int], list[dict]],
) -> list[int]:
    """Return physical member buckets that can affect one logical source."""
    buckets = {
        bucket_start
        for member_id, bucket_start in specs_by_member_bucket
        if member_id in source.members
    }
    return sorted(buckets)


def logical_source_member_specs(
    source: SourceDefinition,
    bucket_start: int,
    specs_by_member_bucket: dict[tuple[str, int], list[dict]],
) -> tuple[list[dict], list[str]]:
    """Return present physical specs and missing members for one logical bucket."""
    present_specs = []
    missing_members = []
    for member_id in source.members:
        candidates = specs_by_member_bucket.get((member_id, bucket_start), [])
        usable = candidates if len(source.members) == 1 else [
            spec
            for spec in candidates
            if not spec.get('gap')
        ]
        if usable:
            present_specs.extend(usable)
        else:
            missing_members.append(member_id)
    return present_specs, missing_members


def logical_nfcapd_gap_locator(source_id: str, bucket_start: int) -> str:
    """Return the synthetic logical gap locator for an empty logical source bucket."""
    timestamp = datetime.fromtimestamp(bucket_start, PIPELINE_TIMEZONE)
    return f"gap://nfcapd/{source_id}/{timestamp.strftime('%Y%m%d%H%M')}"


def nfcapd_logical_bucket_processed(
    conn: sqlite3.Connection,
    source_id: str,
    bucket_start: int,
    input_locators: list[str],
) -> bool:
    """Return true when the logical bucket was processed from exactly these inputs."""
    if not input_locators:
        return False
    init_processed_inputs_table(conn)
    rows = conn.execute(
        """
        SELECT input_locator
        FROM processed_inputs
        WHERE input_kind = 'nfcapd'
          AND source_id = ?
          AND bucket_start = ?
          AND status = 'processed'
        """,
        (source_id, bucket_start),
    ).fetchall()
    return {row[0] for row in rows} == set(input_locators)


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


def parse_optional_config_time(raw_value: object) -> int | None:
    """Parse an optional local timestamp string to epoch seconds."""
    if raw_value in (None, ''):
        return None
    raw_text = str(raw_value)
    for datetime_format in ('%Y-%m-%dT%H:%M', '%Y-%m-%d %H:%M', '%Y-%m-%dT%H:%M:%S', '%Y-%m-%d %H:%M:%S'):
        try:
            parsed = datetime.strptime(raw_text, datetime_format)
            return int(parsed.replace(tzinfo=PIPELINE_TIMEZONE).timestamp())
        except ValueError:
            continue
    raise ValueError(f'Invalid time {raw_text!r}; expected YYYY-MM-DDTHH:MM.')


def local_midnight_epoch(day: datetime) -> int:
    """Return local midnight epoch seconds for a naive date value."""
    return int(day.replace(tzinfo=PIPELINE_TIMEZONE).timestamp())


def validate_aggregate_safe_time_window(
    start_bucket: int | None,
    end_bucket: int | None,
    *,
    start_date: datetime,
    end_date: datetime,
) -> None:
    """Reject partial time windows that cannot produce complete aggregate rows."""
    for label, value in (('start_time', start_bucket), ('end_time', end_bucket)):
        if value is None:
            continue
        for granularity, seconds in AGGREGATE_GRANULARITY_SECONDS:
            if floor_bucket_start(value, seconds) != value:
                raise ValueError(
                    f'{label} must align to a {granularity} boundary so aggregate rows stay complete.'
                )

    selected_start = local_midnight_epoch(start_date)
    selected_end = local_midnight_epoch(end_date + timedelta(days=1))
    window_start = selected_start if start_bucket is None else start_bucket
    window_end = selected_end if end_bucket is None else end_bucket

    if window_start < selected_start:
        raise ValueError('start_time must be on or after the selected start_date')
    if window_end > selected_end:
        raise ValueError('end_time must be on or before the selected end_date window')
    if window_end <= window_start:
        raise ValueError('time window must be non-empty')


def filter_input_specs_by_bucket_window(
    input_specs: list[dict],
    *,
    start_bucket: int | None,
    end_bucket: int | None,
) -> list[dict]:
    """Filter nfcapd specs to a half-open bucket window."""
    filtered = []
    for spec in input_specs:
        bucket_start = int(spec.get('bucket_start') or parse_nfcapd_bucket_start(str(spec['path'])))
        if start_bucket is not None and bucket_start < start_bucket:
            continue
        if end_bucket is not None and bucket_start >= end_bucket:
            continue
        filtered.append(spec)
    return filtered


def build_dataset_tree_config(
    *,
    dataset_id: str,
    start_date: str,
    end_date: str | None = None,
    database_path: str | Path | None = None,
    maad_bin: str | Path = DEFAULT_MAAD_BIN,
    max_workers: int = DEFAULT_MAX_WORKERS,
    start_time: str | None = None,
    end_time: str | None = None,
    force: bool = False,
) -> dict:
    """Build a nfcapd_tree config from datasets.json."""
    from common import get_dataset_config, list_dataset_sources

    max_workers = validate_max_workers(max_workers)
    dataset = get_dataset_config(dataset_id)
    db_path = Path(database_path) if database_path is not None else Path(dataset['db_path'])
    tree_input = {
        'input_kind': 'nfcapd_tree',
        'root_path': dataset['root_path'],
        'start_date': start_date,
    }
    if dataset.get('sources'):
        tree_input['sources'] = dataset['sources']
    else:
        tree_input['source_ids'] = list_dataset_sources(dataset_id)
    if end_date is not None:
        tree_input['end_date'] = end_date
    if start_time is not None:
        tree_input['start_time'] = start_time
    if end_time is not None:
        tree_input['end_time'] = end_time
    if force:
        tree_input['force'] = True
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
    """Process explicit input specs into canonical aggregate tables."""
    init_processed_inputs_table(conn)
    init_stats_tables(conn)

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
    canonical_buckets = []

    for payload in iter_input_payloads(tasks, max_workers):
        write_input_payload(conn, payload, mark_processed=False)
        processed_buckets.extend(payload['processed_buckets'])
        canonical_buckets.extend(payload.get('canonical_buckets', []))

    write_aggregate_rows(
        conn,
        canonical_buckets,
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
    aggregate_buckets: dict[tuple[str, str, int], StatisticalBucket] = {}

    for payload in iter_input_payloads(tasks, max_workers):
        write_input_payload(conn, payload, mark_processed=False)
        processed_buckets.extend(payload['processed_buckets'])
        for raw_bucket in payload.get('canonical_buckets', []):
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


def process_nfcapd_logical_bucket_jobs(
    conn: sqlite3.Connection,
    jobs: list[dict],
    *,
    maad_bin: str | Path = DEFAULT_MAAD_BIN,
    maad_backend: str = 'subprocess',
    maad_workers: int = 1,
    max_workers: int = DEFAULT_MAX_WORKERS,
    run_maad: bool = True,
) -> None:
    """Process logical nfcapd source buckets with bounded raw payload retention."""
    init_processed_inputs_table(conn)
    init_stats_tables(conn)

    jobs = sorted(jobs, key=lambda job: (int(job['bucket_start']), str(job['source_id'])))
    processed_buckets = []
    aggregate_buckets: dict[tuple[str, str, int], StatisticalBucket] = {}
    pending_structure_raw_buckets = []
    structure_batch_size = max(1, maad_workers) * 4
    bucket_window_size = max(1, max_workers // 2)

    for window_jobs in iter_logical_job_windows(jobs, bucket_window_size):
        member_raw_buckets = build_member_raw_buckets_for_jobs(
            window_jobs,
            maad_bin=maad_bin,
            maad_backend=maad_backend,
            maad_workers=maad_workers,
            max_workers=max_workers,
        )
        for job in window_jobs:
            raw_bucket = process_logical_nfcapd_job(
                conn,
                job,
                member_raw_buckets,
                maad_bin=maad_bin,
                maad_backend=maad_backend,
                aggregate_buckets=aggregate_buckets,
                processed_buckets=processed_buckets,
                delete_existing=True,
            )
            if run_maad:
                pending_structure_raw_buckets.append(raw_bucket)
                if len(pending_structure_raw_buckets) >= structure_batch_size:
                    flush_5m_address_structure_raw_buckets(
                        conn,
                        pending_structure_raw_buckets,
                        maad_bin,
                        maad_backend,
                        maad_workers,
                    )
                    pending_structure_raw_buckets.clear()
        aggregate_cutoff = max(int(job['bucket_start']) for job in window_jobs)
        ready_aggregate_keys = [
            key for key, bucket in aggregate_buckets.items() if bucket.key.bucket_end <= aggregate_cutoff
        ]
        flush_streaming_aggregate_buckets(
            conn,
            aggregate_buckets,
            ready_aggregate_keys,
            maad_bin,
            maad_backend,
            aggregate_maad_worker_count(max(max_workers, maad_workers)),
            run_maad,
            delete_existing=True,
        )

    if pending_structure_raw_buckets:
        flush_5m_address_structure_raw_buckets(
            conn,
            pending_structure_raw_buckets,
            maad_bin,
            maad_backend,
            maad_workers,
        )

    flush_streaming_aggregate_buckets(
        conn,
        aggregate_buckets,
        list(aggregate_buckets),
        maad_bin,
        maad_backend,
        aggregate_maad_worker_count(max(max_workers, maad_workers)),
        run_maad,
        delete_existing=True,
    )
    with conn:
        mark_processed_buckets(conn, processed_buckets)


def iter_logical_job_windows(jobs: list[dict], bucket_window_size: int) -> Iterable[list[dict]]:
    """Yield sorted logical jobs grouped by bounded bucket-start windows."""
    jobs_by_bucket = defaultdict(list)
    for job in jobs:
        jobs_by_bucket[int(job['bucket_start'])].append(job)
    bucket_starts = sorted(jobs_by_bucket)
    for index in range(0, len(bucket_starts), bucket_window_size):
        window_starts = bucket_starts[index:index + bucket_window_size]
        yield [
            job
            for bucket_start in window_starts
            for job in sorted(jobs_by_bucket[bucket_start], key=lambda item: str(item['source_id']))
        ]


def build_member_raw_buckets_for_jobs(
    jobs: list[dict],
    *,
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
    max_workers: int,
) -> dict[str, CanonicalBucket]:
    """Build physical member raw buckets needed by a bounded logical job window."""
    member_specs_by_locator = {}
    for job in jobs:
        for spec in job['member_specs']:
            member_specs_by_locator[str(spec['path'])] = spec
    tasks = [
        (spec, str(maad_bin), maad_backend, maad_workers, False)
        for _, spec in sorted(member_specs_by_locator.items())
    ]
    member_raw_buckets = {}
    for payload in iter_input_payloads(tasks, max_workers):
        processed_bucket = payload['processed_buckets'][0]
        member_raw_buckets[processed_bucket['input_locator']] = payload['canonical_buckets'][0]
    return member_raw_buckets


def process_logical_nfcapd_job(
    conn: sqlite3.Connection,
    job: dict,
    member_raw_buckets: dict[str, CanonicalBucket],
    *,
    maad_bin: str | Path,
    maad_backend: str,
    aggregate_buckets: dict[tuple[str, str, int], StatisticalBucket],
    processed_buckets: list[dict],
    delete_existing: bool,
) -> CanonicalBucket:
    """Write one logical nfcapd job and feed streaming aggregates."""
    payload = build_logical_nfcapd_bucket_payload(
        job,
        member_raw_buckets,
        maad_bin=maad_bin,
        maad_backend=maad_backend,
        run_maad=False,
    )
    write_input_payload(conn, payload, mark_processed=False, delete_existing=delete_existing)
    processed_buckets.extend(payload['processed_buckets'])
    raw_bucket = payload['canonical_buckets'][0]
    add_raw_bucket_to_streaming_aggregates(aggregate_buckets, raw_bucket)
    return raw_bucket


def flush_5m_address_structure_raw_buckets(
    conn: sqlite3.Connection,
    raw_buckets: list[CanonicalBucket],
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
) -> None:
    """Write 5m address-structure rows for a bounded raw-bucket batch."""
    if not raw_buckets:
        return
    address_structure_rows = build_address_structure_rows_from_raw_buckets(
        raw_buckets,
        maad_bin,
        maad_backend,
        maad_workers,
    )
    with conn:
        insert_address_structure_stats_rows(conn, address_structure_rows)


def build_logical_nfcapd_bucket_payload(
    job: dict,
    member_raw_buckets: dict[str, CanonicalBucket],
    *,
    maad_bin: str | Path,
    maad_backend: str,
    run_maad: bool,
) -> dict:
    """Build one logical nfcapd bucket by summing and unioning member payloads."""
    source_id = str(job['source_id'])
    bucket_start = int(job['bucket_start'])
    bucket_end = int(job['bucket_end'])
    raw_buckets = [
        member_raw_buckets[str(spec['path'])]
        for spec in job['member_specs']
    ]
    if not raw_buckets:
        return build_nfcapd_gap_payload(
            logical_nfcapd_gap_locator(source_id, bucket_start),
            source_id,
            bucket_start,
            run_maad=run_maad,
        )
    canonical_bucket = merge_raw_buckets_for_source(
        source_id=source_id,
        bucket_start=bucket_start,
        bucket_end=bucket_end,
        raw_buckets=raw_buckets,
    )
    rows = canonical_bucket_rows(canonical_bucket)
    return {
        'processed_buckets': [
            {
                'input_kind': 'nfcapd',
                'input_locator': str(spec['path']),
                'source_id': source_id,
                'bucket_start': bucket_start,
                'bucket_end': bucket_end,
            }
            for spec in job['member_specs']
        ],
        'traffic_rows': rows['traffic_rows'],
        'protocol_rows': rows['protocol_rows'],
        'address_count_rows': rows['address_count_rows'],
        'address_structure_rows': build_address_structure_rows_from_raw_buckets(
            [canonical_bucket],
            maad_bin,
            maad_backend,
            1,
        )
        if run_maad
        else [],
        'canonical_buckets': [canonical_bucket],
    }


def merge_raw_buckets_for_source(
    *,
    source_id: str,
    bucket_start: int,
    bucket_end: int,
    raw_buckets: list[CanonicalBucket],
) -> CanonicalBucket:
    """Merge physical member buckets while retargeting their logical source."""
    bucket = StatisticalBucket(BucketKey(source_id, '5m', bucket_start, bucket_end))
    for raw_bucket in raw_buckets:
        bucket.include(raw_bucket)
    return bucket.finish()


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
            print(f'[pipeline] Skip CSV input already processed: {input_locator}')
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

        print(f'[pipeline] CSV start: {input_locator}')
        active_buckets: dict[tuple[str, int, int], StatisticalBucket] = {}
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
                    f'[pipeline] CSV rows={rows_seen} active_buckets={len(active_buckets)} '
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
            f'[pipeline] CSV complete: rows={rows_seen} buckets={len(processed_buckets)} '
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
    init_processed_inputs_table(conn)
    rows = conn.execute(
        """
        SELECT status, COUNT(*)
        FROM processed_inputs
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
    return set(CSV_ARROW_REQUIRED_COLUMN_KEYS).issubset(config.columns)


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
    aggregate_buckets: dict[tuple[str, str, int], StatisticalBucket],
) -> None:
    """Stream a headerless CSV input through PyArrow grouped batches."""
    input_locator = str(spec['path'])
    active_arrow_buckets: dict[int, list] = {}
    max_bucket_start: int | None = None
    rows_seen = 0

    print(f'[pipeline] CSV start: {input_locator}')
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
                f'[pipeline] CSV rows={rows_seen} active_buckets={len(active_arrow_buckets)} '
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
        f'[pipeline] CSV complete: rows={rows_seen} buckets={len(processed_buckets)} '
        f'input={input_locator}'
    )


def iter_csv_arrow_batches(input_locator: str, config: CsvSourceConfig):
    """Yield PyArrow record batches for one configured headerless CSV input."""
    import pyarrow as pa
    import pyarrow.csv as arrow_csv

    include_columns = [config.columns[key] for key in CSV_ARROW_REQUIRED_COLUMN_KEYS]
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
    src_tos_column = config.columns['src_tos']
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
            'src_tos': pc.cast(batch[src_tos_column], pa.int64()),
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
    aggregate_buckets: dict[tuple[str, str, int], StatisticalBucket],
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
) -> StatisticalBucket:
    """Build a statistical bucket from Arrow chunks for one 5-minute interval."""
    import pyarrow as pa

    table = pa.concat_tables(tables)
    bucket = StatisticalBucket(BucketKey(source_id, '5m', bucket_start, bucket_start + 300))
    for row in table.to_pylist():
        bucket.add(
            FlowFact(
                ip_version=int(row['ip_version']),
                src_ip=row['src_ip'],
                dst_ip=row['dst_ip'],
                protocol=parse_arrow_protocol(row['protocol'], config),
                packets=int(row['packets']),
                bytes_count=int(row['bytes']),
                src_tos=int(row['src_tos']),
            )
        )
    return bucket


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
    aggregate_buckets: dict[tuple[str, str, int], StatisticalBucket],
) -> None:
    """Stream a headerless CSV input directly into bucket accumulators."""
    input_locator = str(spec['path'])
    field_indexes = build_field_indexes(config, input_locator)
    active_buckets: dict[tuple[str, int, int], StatisticalBucket] = {}
    max_bucket_start: int | None = None
    rows_seen = 0

    print(f'[pipeline] CSV start: {input_locator}')
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
                f'[pipeline] CSV rows={rows_seen} active_buckets={len(active_buckets)} '
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
        f'[pipeline] CSV complete: rows={rows_seen} buckets={len(processed_buckets)} '
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
    buckets: dict[tuple[str, int, int], StatisticalBucket],
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
    src_tos = extract_csv_value_int(values, config, field_indexes, 'src_tos')

    key = (source_id, bucket_start, bucket_end)
    bucket = buckets.setdefault(
        key,
        StatisticalBucket(BucketKey(source_id, '5m', bucket_start, bucket_end)),
    )
    bucket.add(
        FlowFact(
            ip_version=ip_version,
            src_ip=src_ip,
            dst_ip=dst_ip,
            protocol=protocol,
            packets=packets,
            bytes_count=bytes_count,
            src_tos=src_tos,
        )
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
    active_buckets: dict[tuple[str, int, int], StatisticalBucket],
    keys: list[tuple[str, int, int]],
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
    run_maad: bool,
    processed_buckets: list[dict],
    aggregate_buckets: dict[tuple[str, str, int], StatisticalBucket],
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
    bucket_values: list[StatisticalBucket],
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
    run_maad: bool,
    processed_buckets: list[dict],
    aggregate_buckets: dict[tuple[str, str, int], StatisticalBucket],
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
    for raw_bucket in payload['canonical_buckets']:
        add_raw_bucket_to_streaming_aggregates(aggregate_buckets, raw_bucket)
    ready_aggregate_keys = [
        key for key, bucket in aggregate_buckets.items() if bucket.key.bucket_end <= aggregate_cutoff
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
    bucket_values: list[StatisticalBucket],
) -> list[StatisticalBucket]:
    """Drop CSV buckets already marked processed during a retry."""
    processed_keys = {
        (row[0], row[1])
        for row in conn.execute(
            """
            SELECT source_id, bucket_start
            FROM processed_inputs
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
        if (bucket.key.source_id, bucket.key.bucket_start) not in processed_keys
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
        FROM processed_inputs
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
    bucket_values: list[StatisticalBucket],
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
    run_maad: bool,
) -> dict:
    """Build DB insert payloads from completed bucket accumulators."""
    canonical_buckets = [bucket.finish() for bucket in bucket_values]
    rows = [canonical_bucket_rows(bucket) for bucket in canonical_buckets]
    return {
        'processed_buckets': [
            {
                'input_kind': input_kind,
                'input_locator': input_locator,
                'source_id': bucket.key.source_id,
                'bucket_start': bucket.key.bucket_start,
                'bucket_end': bucket.key.bucket_end,
            }
            for bucket in bucket_values
        ],
        'traffic_rows': [row for payload in rows for row in payload['traffic_rows']],
        'protocol_rows': [row for payload in rows for row in payload['protocol_rows']],
        'address_count_rows': [row for payload in rows for row in payload['address_count_rows']],
        'address_structure_rows': build_address_structure_rows_from_raw_buckets(
            canonical_buckets,
            maad_bin,
            maad_backend,
            maad_workers,
        )
        if run_maad
        else [],
        'canonical_buckets': canonical_buckets,
    }


def add_raw_bucket_to_streaming_aggregates(
    aggregate_buckets: dict[tuple[str, str, int], StatisticalBucket],
    raw_bucket: CanonicalBucket,
) -> None:
    """Include one completed 5m bucket in bounded aggregate builders."""
    for granularity, seconds in AGGREGATE_GRANULARITY_SECONDS:
        bucket_start = floor_bucket_start(raw_bucket.key.bucket_start, seconds)
        key = (raw_bucket.key.source_id, granularity, bucket_start)
        aggregate = aggregate_buckets.setdefault(
            key,
            StatisticalBucket(
                BucketKey(
                    raw_bucket.key.source_id,
                    granularity,
                    bucket_start,
                    next_bucket_start(bucket_start, seconds),
                )
            ),
        )
        aggregate.include(raw_bucket)


def flush_streaming_aggregate_buckets(
    conn: sqlite3.Connection,
    aggregate_buckets: dict[tuple[str, str, int], StatisticalBucket],
    keys: list[tuple[str, str, int]],
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
    run_maad: bool,
    delete_existing: bool = False,
) -> None:
    """Write and discard aggregate buckets that cannot receive more rows."""
    if not keys:
        return
    buckets = [aggregate_buckets.pop(key).finish() for key in sorted(keys)]
    if delete_existing:
        buckets = [bucket for bucket in buckets if bucket.has_complete_five_minute_coverage]
        if not buckets:
            return
    stats_payloads = [canonical_bucket_rows(bucket) for bucket in buckets]
    traffic_rows = [
        row
        for payload in stats_payloads
        for row in payload['traffic_rows']
    ]
    protocol_rows = [
        row
        for payload in stats_payloads
        for row in payload['protocol_rows']
    ]
    address_count_rows = [
        row
        for payload in stats_payloads
        for row in payload['address_count_rows']
    ]
    address_structure_rows = (
        build_address_structure_rows_from_payloads(
            stats_payloads,
            maad_bin,
            maad_backend,
            maad_workers,
        )
        if run_maad
        else []
    )
    with conn:
        if delete_existing:
            delete_aggregate_outputs_for_streaming_buckets(conn, buckets)
        insert_traffic_stats_rows(conn, traffic_rows)
        insert_protocol_stats_rows(conn, protocol_rows)
        insert_address_count_stats_rows(conn, address_count_rows)
        insert_address_structure_stats_rows(conn, address_structure_rows)


def delete_aggregate_outputs_for_streaming_buckets(
    conn: sqlite3.Connection,
    buckets: list[CanonicalBucket],
) -> None:
    """Delete stale aggregate rows for streaming aggregate buckets before rewrite."""
    for bucket in buckets:
        for table_name in (
            'traffic_stats',
            'protocol_stats',
            'address_count_stats',
            'address_structure_stats',
        ):
            conn.execute(
                f'DELETE FROM {table_name} WHERE source_id = ? AND granularity = ? AND bucket_start = ?',
                (bucket.key.source_id, bucket.key.granularity, bucket.key.bucket_start),
            )


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
        canonical_bucket = nfcapd_payload['canonical_bucket']
        return {
            'processed_buckets': [nfcapd_payload['processed_bucket']],
            'traffic_rows': nfcapd_payload['traffic_rows'],
            'protocol_rows': nfcapd_payload['protocol_rows'],
            'address_count_rows': nfcapd_payload['address_count_rows'],
            'address_structure_rows': build_address_structure_rows_from_raw_buckets(
                [canonical_bucket],
                maad_bin,
                maad_backend,
                maad_workers,
            )
            if run_maad
            else [],
            'canonical_buckets': [canonical_bucket],
        }

    buckets = accumulate_input_buckets(iter_input_rows(spec))
    bucket_values = [buckets[key] for key in sorted(buckets)]

    canonical_buckets = [bucket.finish() for bucket in bucket_values]
    rows = [canonical_bucket_rows(bucket) for bucket in canonical_buckets]
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
        'traffic_rows': [row for payload in rows for row in payload['traffic_rows']],
        'protocol_rows': [row for payload in rows for row in payload['protocol_rows']],
        'address_count_rows': [row for payload in rows for row in payload['address_count_rows']],
        'address_structure_rows': build_address_structure_rows_from_raw_buckets(
            canonical_buckets,
            maad_bin,
            maad_backend,
            maad_workers,
        )
        if run_maad
        else [],
        'canonical_buckets': canonical_buckets,
    }


def write_input_payload(
    conn: sqlite3.Connection,
    payload: dict,
    *,
    mark_processed: bool = True,
    delete_existing: bool = False,
) -> None:
    """Persist a worker payload. SQLite writes remain in the parent process."""
    processed_buckets = payload['processed_buckets']
    if not processed_buckets:
        return
    with conn:
        if delete_existing:
            delete_5m_outputs_for_processed_buckets(conn, processed_buckets)

        for bucket in processed_buckets:
            upsert_input_bucket(conn, **bucket)

        insert_traffic_stats_rows(conn, payload.get('traffic_rows', []))
        insert_protocol_stats_rows(conn, payload.get('protocol_rows', []))
        insert_address_count_stats_rows(conn, payload.get('address_count_rows', []))
        insert_address_structure_stats_rows(conn, payload.get('address_structure_rows', []))

        if mark_processed:
            mark_processed_buckets(conn, processed_buckets)


def delete_5m_outputs_for_processed_buckets(conn: sqlite3.Connection, processed_buckets: list[dict]) -> None:
    """Delete stale 5m rows before rewriting logical source buckets."""
    seen = set()
    for bucket in processed_buckets:
        key = (bucket['source_id'], bucket['bucket_start'])
        if key in seen:
            continue
        seen.add(key)
        delete_5m_outputs(conn, source_id=bucket['source_id'], bucket_start=bucket['bucket_start'])


def delete_5m_outputs(conn: sqlite3.Connection, *, source_id: str, bucket_start: int) -> None:
    """Delete one source/bucket from all 5m output tables and input tracking."""
    conn.execute(
        """
        DELETE FROM processed_inputs
        WHERE input_kind = 'nfcapd' AND source_id = ? AND bucket_start = ?
        """,
        (source_id, bucket_start),
    )
    for table_name in (
        'traffic_stats',
        'protocol_stats',
        'address_count_stats',
        'address_structure_stats',
    ):
        conn.execute(
            f"DELETE FROM {table_name} WHERE source_id = ? AND granularity = '5m' AND bucket_start = ?",
            (source_id, bucket_start),
        )


def mark_processed_buckets(conn: sqlite3.Connection, processed_buckets: list[dict]) -> None:
    """Mark input buckets processed after all outputs are written."""
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
    raw_buckets: list[CanonicalBucket],
    maad_bin: str | Path,
    max_workers: int,
    *,
    maad_backend: str = 'subprocess',
    maad_workers: int = 1,
    run_maad: bool = True,
    delete_existing: bool = False,
) -> None:
    """Write 30m, 1h, and 1d aggregate rows from canonical 5m buckets."""
    if not raw_buckets:
        return
    stats_payloads = build_aggregate_stats_payloads(raw_buckets)
    traffic_rows = [
        row
        for payload in stats_payloads
        for row in payload['traffic_rows']
    ]
    protocol_rows = [
        row
        for payload in stats_payloads
        for row in payload['protocol_rows']
    ]
    address_count_rows = [
        row
        for payload in stats_payloads
        for row in payload['address_count_rows']
    ]
    address_structure_rows = (
        build_address_structure_rows_from_payloads(
            stats_payloads,
            maad_bin,
            maad_backend,
            maad_workers,
        )
        if run_maad
        else []
    )
    with conn:
        if delete_existing:
            delete_aggregate_outputs_for_raw_buckets(conn, raw_buckets)
        insert_traffic_stats_rows(conn, traffic_rows)
        insert_protocol_stats_rows(conn, protocol_rows)
        insert_address_count_stats_rows(conn, address_count_rows)
        insert_address_structure_stats_rows(conn, address_structure_rows)


def delete_aggregate_outputs_for_raw_buckets(
    conn: sqlite3.Connection,
    raw_buckets: list[CanonicalBucket],
) -> None:
    """Delete stale aggregate rows affected by rewritten raw buckets."""
    keys = set()
    for raw in raw_buckets:
        for granularity, seconds in AGGREGATE_GRANULARITY_SECONDS:
            keys.add(
                (
                    raw.key.source_id,
                    granularity,
                    floor_bucket_start(raw.key.bucket_start, seconds),
                )
            )

    for source_id, granularity, bucket_start in sorted(keys):
        for table_name in (
            'traffic_stats',
            'protocol_stats',
            'address_count_stats',
            'address_structure_stats',
        ):
            conn.execute(
                f'DELETE FROM {table_name} WHERE source_id = ? AND granularity = ? AND bucket_start = ?',
                (source_id, granularity, bucket_start),
            )


def build_aggregate_stats_payloads(raw_buckets: list[CanonicalBucket]) -> list[dict]:
    """Build aggregate payloads for every affected granularity bucket."""
    buckets = defaultdict(list)
    for raw in raw_buckets:
        for granularity, seconds in AGGREGATE_GRANULARITY_SECONDS:
            bucket_start = floor_bucket_start(raw.key.bucket_start, seconds)
            bucket_end = next_bucket_start(bucket_start, seconds)
            buckets[(raw.key.source_id, granularity, bucket_start, bucket_end)].append(raw)

    payloads = []
    for (source_id, granularity, bucket_start, bucket_end), children in sorted(buckets.items()):
        aggregate = StatisticalBucket(BucketKey(source_id, granularity, bucket_start, bucket_end))
        for child in children:
            aggregate.include(child)
        payloads.append(canonical_bucket_rows(aggregate.finish()))
    return payloads


def build_address_structure_rows_from_payloads(
    payloads: list[dict],
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
) -> list[dict]:
    """Build address_structure_stats rows from raw address-set payloads."""
    entries = [
        entry
        for payload in payloads
        for entry in payload['address_sets']
        if entry['ip_version'] == 4
    ]
    return build_address_structure_rows_from_address_sets(
        entries,
        maad_bin,
        maad_backend,
        maad_workers,
    )


def build_address_structure_rows_from_raw_buckets(
    raw_buckets: list[CanonicalBucket],
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
) -> list[dict]:
    """Build 5m address_structure_stats rows from raw bucket address sets."""
    entries = [
        entry
        for raw_bucket in raw_buckets
        for entry in canonical_bucket_rows(raw_bucket)['address_sets']
        if entry['ip_version'] == 4
    ]
    return build_address_structure_rows_from_address_sets(
        entries,
        maad_bin,
        maad_backend,
        maad_workers,
    )


def build_address_structure_rows_from_address_sets(
    entries: list[dict],
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
) -> list[dict]:
    """Run MAAD for address-set entries."""
    tasks = [
        {
            'maad_bin': str(maad_bin),
            'maad_backend': maad_backend,
            **entry,
        }
        for entry in entries
    ]
    if not tasks:
        return []
    if maad_workers > 1 and len(tasks) > 1:
        with Pool(processes=maad_workers) as pool:
            results = list(pool.imap_unordered(process_address_structure_task, tasks, chunksize=1))
    else:
        results = [process_address_structure_task(task) for task in tasks]
    return [row for rows in results for row in rows]


def process_address_structure_task(task: dict) -> list[dict]:
    """Run MAAD for one address set."""
    timeout_seconds = maad_timeout_seconds(str(task['granularity']))
    result = run_maad_for_addresses(
        task['maad_bin'],
        set(task['addresses']),
        maad_backend=str(task.get('maad_backend', 'subprocess')),
        timeout_seconds=timeout_seconds,
    )
    return build_address_structure_stats_rows(
        source_id=task['source_id'],
        granularity=task['granularity'],
        bucket_start=task['bucket_start'],
        bucket_end=task['bucket_end'],
        ip_version=task['ip_version'],
        src_visibility=task['src_visibility'],
        dst_visibility=task['dst_visibility'],
        address_side=task['address_side'],
        result=result,
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
    """Build zero-valued canonical rows for one bounded missing nfcapd bucket."""
    bucket_end = bucket_start + FIVE_MINUTE_SECONDS
    canonical_bucket = StatisticalBucket(
        BucketKey(source_id, '5m', bucket_start, bucket_end),
        dense=True,
    ).finish()
    rows = canonical_bucket_rows(canonical_bucket)
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
        'traffic_rows': rows['traffic_rows'],
        'protocol_rows': rows['protocol_rows'],
        'address_count_rows': rows['address_count_rows'],
        'address_structure_rows': build_address_structure_rows_from_raw_buckets(
            [canonical_bucket],
            '',
            'python',
            1,
        )
        if run_maad
        else [],
        'canonical_buckets': [canonical_bucket],
    }


def accumulate_input_buckets(rows: Iterable[NormalizedRow]) -> dict[tuple[str, int, int], StatisticalBucket]:
    """Accumulate normalized rows by source and 5-minute bucket."""
    buckets: dict[tuple[str, int, int], StatisticalBucket] = {}
    for row in rows:
        add_row_to_bucket(buckets, row)
    return buckets


def add_row_to_bucket(
    buckets: dict[tuple[str, int, int], StatisticalBucket],
    row: NormalizedRow,
) -> None:
    """Accumulate one normalized row into a bucket map."""
    key = (row.source_id, row.bucket_start, row.bucket_end)
    bucket = buckets.setdefault(
        key,
        StatisticalBucket(BucketKey(row.source_id, '5m', row.bucket_start, row.bucket_end)),
    )
    bucket.add(
        FlowFact(
            ip_version=row.ip_version,
            src_ip=row.src_ip,
            dst_ip=row.dst_ip,
            protocol=row.protocol,
            packets=row.packets,
            bytes_count=row.bytes,
            src_tos=row.src_tos,
        )
    )


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
    """Run the minimal pipeline entrypoint."""
    parser = argparse.ArgumentParser(description='Pipeline processor')
    parser.add_argument('--config', help='Path to the pipeline json config.')
    parser.add_argument('--dataset', help='Dataset id from datasets.json for canonical nfcapd tree input.')
    parser.add_argument('--start-date', help='Start date for --dataset, inclusive, YYYY-MM-DD.')
    parser.add_argument('--end-date', help='End date for --dataset, inclusive, YYYY-MM-DD. Defaults to latest nfcapd day.')
    parser.add_argument('--start-time', help='Optional local half-open window start, YYYY-MM-DDTHH:MM.')
    parser.add_argument('--end-time', help='Optional local half-open window end, YYYY-MM-DDTHH:MM.')
    parser.add_argument('--database-path', help='Override SQLite output path.')
    parser.add_argument('--maad-bin', help='Path to MAAD binary.')
    parser.add_argument('--max-workers', type=int, help='Worker process count.')
    parser.add_argument('--force', action='store_true', help='Rewrite selected nfcapd buckets even when marked processed.')
    args = parser.parse_args()

    if args.config:
        config = load_pipeline_config(args.config)
        try:
            config = apply_cli_config_overrides(
                config,
                database_path=args.database_path,
                maad_bin=args.maad_bin,
                max_workers=args.max_workers,
                force=args.force,
            )
        except ValueError as error:
            parser.error(str(error))
    else:
        if not args.dataset or not args.start_date:
            parser.error('--config or both --dataset and --start-date is required')
        try:
            config = build_dataset_tree_config(
                dataset_id=args.dataset,
                start_date=args.start_date,
                end_date=args.end_date,
                database_path=args.database_path,
                maad_bin=args.maad_bin or DEFAULT_MAAD_BIN,
                max_workers=args.max_workers if args.max_workers is not None else DEFAULT_MAX_WORKERS,
                start_time=args.start_time,
                end_time=args.end_time,
                force=args.force,
            )
        except ValueError as error:
            parser.error(str(error))

    db_path = Path(config['database_path'])
    db_path.parent.mkdir(parents=True, exist_ok=True)

    with sqlite3.connect(db_path) as conn:
        process_pipeline_config(conn, config)


if __name__ == '__main__':
    main()
