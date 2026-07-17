#!/usr/bin/env python3
"""
Pipeline entrypoint.

Processes explicit csv and nfcapd inputs into canonical netflow tables.
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import sqlite3
from collections import defaultdict
from dataclasses import dataclass
from datetime import datetime, timedelta
from multiprocessing import Pool
from pathlib import Path
from typing import Iterable
from zoneinfo import ZoneInfo

from csv_ingest import CsvSourceConfig, load_csv_source_config
from csv_inputs import discover_csv_specs
from csv_scan import CsvBucketReady, CsvScanComplete, scan_csv
from datasets_metadata import init_datasets_table, upsert_dataset_metadata
from flow_selection import FlowSelection
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
from input_revision import (
    ExpectedAbsence,
    FileSnapshot,
    InputRevision,
    capture_csv_input_revision,
    capture_nfcapd_input_revision,
    csv_decoder_fingerprint,
    gap_input_revision,
    nfcapd_decoder_fingerprint,
    revision_for_locator,
    verify_file_snapshot,
)
from pipeline_product import (
    ProductIdentity,
    bind_nfcapd_source_layout,
    bind_product_identity,
)
from processed_inputs import (
    InputBucketRef,
    InputRevisionConflict,
    cached_content_fingerprint,
    clear_incomplete_input_scan,
    complete_input_scan,
    init_processed_inputs_table,
    input_scan_fully_processed,
    mark_input_bucket_status,
    upsert_input_bucket,
)
from statistical_bucket import (
    BucketKey,
    CanonicalBucket,
    StatisticalBucket,
    ZERO_FILL_VISIBILITY_PAIRS,
)
from stats import (
    STATS_TABLE_ADAPTERS,
    STATS_TABLE_NAMES,
    build_address_structure_stats_rows,
    canonical_bucket_rows,
    init_stats_tables,
    delete_stats_bucket_keys,
    insert_stats_payload,
)


DEFAULT_MAAD_BIN = Path(__file__).resolve().parent / 'maad_fast'
DEFAULT_MAX_WORKERS = int(os.environ.get('MAX_WORKERS', '8'))
DEFAULT_AGGREGATE_MAAD_MAX_WORKERS = int(os.environ.get('AGGREGATE_MAAD_MAX_WORKERS', '4'))
PIPELINE_TIMEZONE = ZoneInfo(os.environ.get('NETFLOW_TIMEZONE', 'America/Los_Angeles'))
FIVE_MINUTE_SECONDS = 300
LOGGER = logging.getLogger(__name__)

AGGREGATE_GRANULARITY_SECONDS = (('30m', 1800), ('1h', 3600), ('1d', 86400))
MAAD_TIMEOUT_SECONDS_BY_GRANULARITY = {
    '5m': int(os.environ.get('MAAD_TIMEOUT_5M_SECONDS', '300')),
    '30m': int(os.environ.get('MAAD_TIMEOUT_30M_SECONDS', '600')),
    '1h': int(os.environ.get('MAAD_TIMEOUT_1H_SECONDS', '900')),
    '1d': int(os.environ.get('MAAD_TIMEOUT_1D_SECONDS', '1800')),
}


def current_product_identity(
    *,
    run_maad: bool,
    maad_backend: str,
    selection: FlowSelection = FlowSelection(),
) -> ProductIdentity:
    """Return result semantics for the selected pipeline product."""
    return ProductIdentity.create(
        schema={
            'version': 2,
            'tables': [
                {'name': adapter.table_name, 'version': adapter.schema_version}
                for adapter in STATS_TABLE_ADAPTERS
            ],
        },
        selection=selection.normalized_payload(),
        config={
            'version': 1,
            'timezone': str(PIPELINE_TIMEZONE),
            'maad': {
                'enabled': run_maad,
                'backend': maad_backend,
                'contract_version': 1,
            },
        },
    )


def bind_current_product(
    conn: sqlite3.Connection,
    *,
    run_maad: bool,
    maad_backend: str,
    selection: FlowSelection = FlowSelection(),
) -> None:
    """Bind or validate database-wide result semantics."""
    bind_product_identity(
        conn,
        current_product_identity(
            run_maad=run_maad,
            maad_backend=maad_backend,
            selection=selection,
        ),
        output_table_names=STATS_TABLE_NAMES,
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
    maad_bin = config.get('maad_bin', DEFAULT_MAAD_BIN)
    maad_backend = str(config.get('maad_backend', 'subprocess'))
    maad_workers = int(config.get('maad_workers', 1))
    max_workers = validate_max_workers(int(config.get('max_workers', DEFAULT_MAX_WORKERS)))
    run_maad = bool(config.get('run_maad', True))
    selection = FlowSelection.from_payload(config.get('selection'))
    init_processed_inputs_table(conn)
    init_stats_tables(conn)
    bind_current_product(
        conn,
        run_maad=run_maad,
        maad_backend=maad_backend,
        selection=selection,
    )
    nfcapd_layout_sources = []
    for spec in config['inputs']:
        if str(spec['input_kind']) != 'nfcapd_tree':
            continue
        root_path = Path(spec['root_path'])
        nfcapd_layout_sources.extend(normalize_nfcapd_sources(spec, root_path))
    if nfcapd_layout_sources:
        source_ids = [source.source_id for source in nfcapd_layout_sources]
        if len(source_ids) != len(set(source_ids)):
            raise ValueError('nfcapd_tree inputs define duplicate logical source ids')
        bind_nfcapd_source_layout(
            conn,
            [(source.source_id, source.members) for source in nfcapd_layout_sources],
        )
    init_datasets_table(conn)
    for dataset in config.get('datasets', []):
        upsert_dataset_metadata(conn, dataset)
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
                selection=selection,
                bind_source_layout=False,
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
                selection=selection,
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
            selection=selection,
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
    selection: FlowSelection = FlowSelection(),
    bind_source_layout: bool = True,
) -> None:
    """Process a canonical nfcapd tree one day at a time."""
    init_processed_inputs_table(conn)
    init_stats_tables(conn)
    bind_current_product(
        conn,
        run_maad=run_maad,
        maad_backend=maad_backend,
        selection=selection,
    )
    root_path = Path(spec['root_path'])
    sources = normalize_nfcapd_sources(spec, root_path)
    if bind_source_layout:
        bind_nfcapd_source_layout(
            conn,
            [(source.source_id, source.members) for source in sources],
        )
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
        jobs = build_nfcapd_logical_bucket_jobs(
            conn,
            sources,
            member_specs,
            force=force,
            root_path=root_path,
            selection=selection,
        )
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
    selection: FlowSelection = FlowSelection(),
) -> None:
    """Process configured CSV files under one directory."""
    init_processed_inputs_table(conn)
    init_stats_tables(conn)
    bind_current_product(
        conn,
        run_maad=run_maad,
        maad_backend=maad_backend,
        selection=selection,
    )
    csv_config = load_csv_source_config(spec['mapping_path'])
    input_specs = discover_csv_specs(
        spec['root_path'],
        spec['mapping_path'],
        csv_config,
    )
    for input_spec in input_specs:
        input_spec['_csv_config'] = csv_config
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
        selection=selection,
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
    locator = f"gap://nfcapd/{source_id}/{timestamp.strftime('%Y%m%d%H%M')}"
    expected_path = nfcapd_expected_path(root, source_id, bucket_start)
    return {
        'input_kind': 'nfcapd',
        'path': locator,
        'expected_path': str(expected_path),
        '_absence_snapshot': ExpectedAbsence.capture(expected_path),
        'source_id': source_id,
        'bucket_start': bucket_start,
        'gap': True,
        'input_revision': gap_input_revision('nfcapd', locator),
    }


def nfcapd_expected_path(root: Path, source_id: str, bucket_start: int) -> Path:
    """Return the canonical path expected for one native five-minute bucket."""
    timestamp = datetime.fromtimestamp(bucket_start, PIPELINE_TIMEZONE)
    return (
        root
        / source_id
        / timestamp.strftime('%Y')
        / timestamp.strftime('%m')
        / timestamp.strftime('%d')
        / f"nfcapd.{timestamp.strftime('%Y%m%d%H%M')}"
    )


def prepare_input_specs(
    conn: sqlite3.Connection,
    input_specs: list[dict],
    selection: FlowSelection = FlowSelection(),
) -> list[dict]:
    """Load decoder configuration once and attach exact revisions."""
    init_processed_inputs_table(conn)
    csv_configs: dict[str, CsvSourceConfig] = {}
    prepared = []
    for raw_spec in input_specs:
        spec = dict(raw_spec)
        spec['_flow_selection'] = selection
        input_kind = str(spec['input_kind'])
        locator = str(spec['path'])
        if input_kind == 'csv':
            mapping_path = str(spec['mapping_path'])
            config = spec.get('_csv_config')
            if config is None:
                config = csv_configs.get(mapping_path)
                if config is None:
                    config = load_csv_source_config(mapping_path)
                    csv_configs[mapping_path] = config
            spec['_csv_config'] = config
            snapshot = FileSnapshot.capture(locator)
            cached_content = cached_content_fingerprint(
                conn,
                input_kind='csv',
                input_locator=locator,
                file_snapshot=snapshot,
            )
            if cached_content is None:
                revision, snapshot = capture_csv_input_revision(locator, config)
            else:
                revision = InputRevision.create(
                    input_kind='csv',
                    locator=locator,
                    content_fingerprint=cached_content,
                    decoder_fingerprint=csv_decoder_fingerprint(config),
                )
            spec['input_revision'] = revision
            spec['_file_snapshot'] = snapshot
        elif input_kind == 'nfcapd':
            if spec.get('gap'):
                spec['input_revision'] = gap_input_revision('nfcapd', locator)
                absence = spec.get('_absence_snapshot')
                if absence is None:
                    expected_path = spec.get('expected_path')
                    if expected_path is None and not locator.startswith('gap://'):
                        expected_path = locator
                    if expected_path is None:
                        raise ValueError(
                            'Synthetic nfcapd gap requires an expected_path '
                            'to verify continued absence'
                        )
                    absence = ExpectedAbsence.capture(expected_path)
                spec['_absence_snapshot'] = absence
            else:
                snapshot = FileSnapshot.capture(locator)
                cached_content = cached_content_fingerprint(
                    conn,
                    input_kind='nfcapd',
                    input_locator=locator,
                    file_snapshot=snapshot,
                )
                if cached_content is None:
                    revision, snapshot = capture_nfcapd_input_revision(locator)
                else:
                    revision = InputRevision.create(
                        input_kind='nfcapd',
                        locator=locator,
                        content_fingerprint=cached_content,
                        decoder_fingerprint=nfcapd_decoder_fingerprint(),
                    )
                spec['input_revision'] = revision
                spec['_file_snapshot'] = snapshot
        else:
            raise ValueError(f'Unsupported input_kind: {input_kind}')
        prepared.append(spec)
    return prepared


def build_nfcapd_logical_bucket_jobs(
    conn: sqlite3.Connection,
    sources: list[SourceDefinition],
    member_specs: list[dict],
    *,
    force: bool = False,
    root_path: Path | None = None,
    selection: FlowSelection = FlowSelection(),
) -> list[dict]:
    """Build unprocessed logical source buckets from physical member specs."""
    specs_by_member_bucket: dict[tuple[str, int], list[dict]] = defaultdict(list)
    member_specs = prepare_input_specs(conn, member_specs, selection)
    for spec in member_specs:
        member_id = str(spec['source_id'])
        bucket_start = int(spec.get('bucket_start') or parse_nfcapd_bucket_start(str(spec['path'])))
        specs_by_member_bucket[(member_id, bucket_start)].append(spec)

    jobs = []
    needs_processing = False
    for source in sources:
        for bucket_start in source_candidate_bucket_starts(source, specs_by_member_bucket):
            present_specs, missing_members, missing_absences = logical_source_member_specs(
                source,
                bucket_start,
                specs_by_member_bucket,
                root_path=root_path,
            )

            revisions = (
                [spec['input_revision'] for spec in present_specs]
                if present_specs
                else [
                    gap_input_revision(
                        'nfcapd',
                        logical_nfcapd_gap_locator(source.source_id, bucket_start),
                    )
                ]
            )
            if force or not nfcapd_logical_bucket_processed(
                conn,
                source.source_id,
                bucket_start,
                revisions,
            ):
                needs_processing = True
            jobs.append(
                {
                    '_flow_selection': selection,
                    'source_id': source.source_id,
                    'bucket_start': bucket_start,
                    'bucket_end': bucket_start + FIVE_MINUTE_SECONDS,
                    'member_specs': present_specs,
                    'missing_members': missing_members,
                    'absence_snapshots': [
                        spec['_absence_snapshot']
                        for spec in present_specs
                        if spec.get('gap')
                    ] + missing_absences,
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
    *,
    root_path: Path | None,
) -> tuple[list[dict], list[str], list[ExpectedAbsence]]:
    """Return present physical specs and missing members for one logical bucket."""
    present_specs = []
    missing_members = []
    missing_absences = []
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
            candidate_absences = [
                spec['_absence_snapshot']
                for spec in candidates
                if spec.get('gap')
            ]
            if not candidate_absences:
                if root_path is None:
                    raise ValueError(
                        'root_path is required to prove absent logical nfcapd members'
                    )
                candidate_absences.append(
                    ExpectedAbsence.capture(
                        nfcapd_expected_path(root_path, member_id, bucket_start)
                    )
                )
            missing_absences.extend(candidate_absences)
    return present_specs, missing_members, missing_absences


def logical_nfcapd_gap_locator(source_id: str, bucket_start: int) -> str:
    """Return the synthetic logical gap locator for an empty logical source bucket."""
    timestamp = datetime.fromtimestamp(bucket_start, PIPELINE_TIMEZONE)
    return f"gap://nfcapd/{source_id}/{timestamp.strftime('%Y%m%d%H%M')}"


def nfcapd_logical_bucket_processed(
    conn: sqlite3.Connection,
    source_id: str,
    bucket_start: int,
    input_revisions: list[InputRevision],
) -> bool:
    """Return true when the logical bucket was processed from exactly these inputs."""
    if not input_revisions:
        return False
    rows = conn.execute(
        """
        SELECT input_locator, revision_fingerprint
        FROM processed_inputs
        WHERE input_kind = 'nfcapd'
          AND source_id = ?
          AND bucket_start = ?
          AND status = 'processed'
        """,
        (source_id, bucket_start),
    ).fetchall()
    stored = {(row[0], row[1]) for row in rows}
    requested = {
        (revision.locator, revision.fingerprint)
        for revision in input_revisions
    }
    if {item[0] for item in stored} == {item[0] for item in requested} and stored != requested:
        raise InputRevisionConflict(
            'nfcapd input content or decoder changed at an already processed locator; '
            'rerun with force to rewrite it.'
        )
    return stored == requested


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
    selection: FlowSelection = FlowSelection(),
) -> dict:
    """Build a nfcapd_tree config from datasets.json."""
    from common import get_dataset_config, list_dataset_sources

    max_workers = validate_max_workers(max_workers)
    dataset = get_dataset_config(dataset_id)
    if not selection.is_unrestricted and database_path is None:
        raise ValueError('flow selection requires an explicit --database-path')
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
        'selection': selection.normalized_payload(),
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
    selection: FlowSelection = FlowSelection(),
) -> None:
    """Process explicit input specs into canonical aggregate tables."""
    init_processed_inputs_table(conn)
    init_stats_tables(conn)
    bind_current_product(
        conn,
        run_maad=run_maad,
        maad_backend=maad_backend,
        selection=selection,
    )

    input_specs = prepare_input_specs(conn, input_specs, selection)
    csv_specs = [spec for spec in input_specs if str(spec['input_kind']) == 'csv']
    if csv_specs:
        process_csv_input_specs_streaming(
            conn,
            csv_specs,
            maad_bin=maad_bin,
            maad_backend=maad_backend,
            maad_workers=maad_workers,
            run_maad=run_maad,
        )
        input_specs = [spec for spec in input_specs if str(spec['input_kind']) != 'csv']
        if not input_specs:
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
    current_run_keys: set[tuple[str, int]] = set()

    for payload in iter_input_payloads(tasks, max_workers):
        raw_buckets = payload.get('canonical_buckets', [])
        allowed_input_owners = {
            (
                str(item['input_kind']),
                str(item['input_locator']),
                item['input_revision'].fingerprint,
            )
            for item in payload['processed_buckets']
        }
        for raw_bucket in raw_buckets:
            reject_overlapping_canonical_bucket(
                conn,
                raw_bucket,
                allowed_input_owners=allowed_input_owners,
            )
            reject_incomplete_persisted_aggregate(conn, raw_bucket, current_run_keys)
        write_input_payload(conn, payload, mark_processed=False)
        processed_buckets.extend(payload['processed_buckets'])
        for raw_bucket in raw_buckets:
            add_raw_bucket_to_streaming_aggregates(aggregate_buckets, raw_bucket)
            current_run_keys.add((raw_bucket.key.source_id, raw_bucket.key.bucket_start))

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
    if not jobs:
        return
    selection = logical_jobs_flow_selection(jobs)
    init_processed_inputs_table(conn)
    init_stats_tables(conn)
    bind_current_product(
        conn,
        run_maad=run_maad,
        maad_backend=maad_backend,
        selection=selection,
    )

    jobs = sorted(jobs, key=lambda job: (int(job['bucket_start']), str(job['source_id'])))
    processed_buckets = []
    aggregate_buckets: dict[tuple[str, str, int], StatisticalBucket] = {}
    pending_structure_raw_buckets = []
    current_run_keys: set[tuple[str, int]] = set()
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
                current_run_keys=current_run_keys,
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


def logical_jobs_flow_selection(jobs: list[dict]) -> FlowSelection:
    """Return the one selection identity carried by all logical jobs and members."""
    expected: FlowSelection | None = None
    for job_index, job in enumerate(jobs):
        selection = job.get('_flow_selection')
        if not isinstance(selection, FlowSelection):
            raise ValueError(
                f'Logical nfcapd job {job_index} is missing a canonical FlowSelection identity'
            )
        if expected is None:
            expected = selection
        elif selection != expected:
            raise ValueError('Logical nfcapd jobs contain inconsistent FlowSelection identities')

        for member_index, member_spec in enumerate(job.get('member_specs', [])):
            member_selection = member_spec.get('_flow_selection')
            if not isinstance(member_selection, FlowSelection):
                raise ValueError(
                    f'Logical nfcapd job {job_index} member {member_index} '
                    'is missing a canonical FlowSelection identity'
                )
            if member_selection != selection:
                raise ValueError(
                    f'Logical nfcapd job {job_index} contains inconsistent '
                    'FlowSelection identities'
                )

    assert expected is not None
    return expected


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
    current_run_keys: set[tuple[str, int]],
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
    raw_bucket = payload['canonical_buckets'][0]
    reject_overlapping_canonical_bucket(
        conn,
        raw_bucket,
        allowed_input_owners={
            (
                str(item['input_kind']),
                str(item['input_locator']),
                item['input_revision'].fingerprint,
            )
            for item in payload['processed_buckets']
        },
        replaceable_input_kinds={'nfcapd'} if delete_existing else None,
    )
    reject_incomplete_persisted_aggregate(conn, raw_bucket, current_run_keys)
    write_input_payload(conn, payload, mark_processed=False, delete_existing=delete_existing)
    processed_buckets.extend(payload['processed_buckets'])
    add_raw_bucket_to_streaming_aggregates(aggregate_buckets, raw_bucket)
    current_run_keys.add((raw_bucket.key.source_id, raw_bucket.key.bucket_start))
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
        insert_stats_payload(
            conn,
            {'address_structure_rows': address_structure_rows},
            table_names=('address_structure_stats',),
        )


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
        payload = build_nfcapd_gap_payload(
            logical_nfcapd_gap_locator(source_id, bucket_start),
            source_id,
            bucket_start,
            run_maad=run_maad,
            input_revision=gap_input_revision(
                'nfcapd',
                logical_nfcapd_gap_locator(source_id, bucket_start),
            ),
        )
        payload['absence_snapshots'] = job.get('absence_snapshots', [])
        for bucket in payload['processed_buckets']:
            bucket['_absence_snapshots'] = payload['absence_snapshots']
        return payload
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
                'input_revision': spec['input_revision'],
                'file_snapshot': spec.get('_file_snapshot'),
                '_absence_snapshots': job.get('absence_snapshots', []),
            }
            for spec in job['member_specs']
        ],
        'absence_snapshots': job.get('absence_snapshots', []),
        'traffic_rows': rows['traffic_rows'],
        'protocol_rows': rows['protocol_rows'],
        'address_count_rows': rows['address_count_rows'],
        'port_count_rows': rows['port_count_rows'],
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


def process_csv_input_specs_streaming(
    conn: sqlite3.Connection,
    input_specs: list[dict],
    *,
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
    run_maad: bool,
) -> None:
    """Consume the deep CSV scan interface and publish its canonical buckets."""
    processed_buckets = []
    aggregate_buckets: dict[tuple[str, str, int], StatisticalBucket] = {}
    completion_events: list[tuple[CsvScanComplete, InputRevision, FileSnapshot]] = []
    published_through: dict[str, int] = {}
    current_run_keys: set[tuple[str, int]] = set()
    started_scans: list[str] = []
    try:
        # Cross-file order is not a publication contract. A stable scan order
        # normalizes callers that discover the same files in a different order.
        for spec in sorted(input_specs, key=lambda item: str(item['path'])):
            scan_locator = str(spec['path'])
            input_revision = spec['input_revision']
            csv_config = spec['_csv_config']
            if csv_input_fully_processed(conn, input_revision):
                print(f'[pipeline] Skip CSV input already processed: {scan_locator}')
                continue
            prepare_csv_scan_retry(conn, scan_locator)
            started_scans.append(scan_locator)
            print(f'[pipeline] CSV start: {scan_locator}')
            bucket_count = 0
            for event in scan_csv(spec, csv_config, spec['_flow_selection']):
                if isinstance(event, CsvScanComplete):
                    completion_events.append(
                        (event, input_revision, spec['_file_snapshot'])
                    )
                    continue
                publish_csv_bucket_ready(
                    conn,
                    event,
                    maad_bin=maad_bin,
                    maad_backend=maad_backend,
                    maad_workers=maad_workers,
                    run_maad=run_maad,
                    processed_buckets=processed_buckets,
                    aggregate_buckets=aggregate_buckets,
                    published_through=published_through,
                    current_run_keys=current_run_keys,
                    input_revision=input_revision,
                )
                bucket_count += 1
            verify_file_snapshot(scan_locator, spec['_file_snapshot'])
            print(
                f'[pipeline] CSV scanned: buckets={bucket_count} input={scan_locator}'
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
        complete_csv_scans(conn, processed_buckets, completion_events)
    except Exception as error:
        for scan_locator in reversed(started_scans):
            try:
                prepare_csv_scan_retry(conn, scan_locator)
            except Exception as cleanup_error:
                error.add_note(
                    f'Cleanup of incomplete CSV scan {scan_locator!r} also failed: '
                    f'{cleanup_error!r}'
                )
        raise


def csv_input_fully_processed(conn: sqlite3.Connection, input_revision: InputRevision) -> bool:
    """Return true only after a successful CSV scan and all scoped publication."""
    return input_scan_fully_processed(
        conn,
        input_kind='csv',
        scan_locator=input_revision.locator,
        input_revision=input_revision,
    )


class AggregatePublicationConflict(ValueError):
    """A cross-input conflict that must not leave partial aggregate publication visible."""


def prepare_csv_scan_retry(
    conn: sqlite3.Connection,
    scan_locator: str,
) -> list[InputBucketRef]:
    """Remove an incomplete scan attempt and every statistical row it could have affected."""
    with conn:
        stale_buckets = clear_incomplete_input_scan(
            conn,
            input_kind='csv',
            scan_locator=scan_locator,
        )
        five_minute_keys = sorted(
            (bucket.source_id, bucket.bucket_start)
            for bucket in stale_buckets
        )
        aggregate_keys = sorted(
            {
                (
                    bucket.source_id,
                    granularity,
                    floor_bucket_start(bucket.bucket_start, seconds),
                )
                for bucket in stale_buckets
                for granularity, seconds in AGGREGATE_GRANULARITY_SECONDS
            }
        )
        delete_stats_bucket_keys(
            conn,
            [
                (source_id, '5m', bucket_start)
                for source_id, bucket_start in five_minute_keys
            ],
        )
        delete_stats_bucket_keys(conn, aggregate_keys)
    return stale_buckets


def publish_csv_bucket_ready(
    conn: sqlite3.Connection,
    event: CsvBucketReady,
    *,
    maad_bin: str | Path,
    maad_backend: str,
    maad_workers: int,
    run_maad: bool,
    processed_buckets: list[dict],
    aggregate_buckets: dict[tuple[str, str, int], StatisticalBucket],
    published_through: dict[str, int],
    current_run_keys: set[tuple[str, int]],
    input_revision: InputRevision,
) -> None:
    """Publish one scanned CSV bucket while preserving scan ownership and provenance."""
    bucket = event.bucket
    previous_start = published_through.get(bucket.key.source_id)
    if previous_start is not None and bucket.key.bucket_start < previous_start:
        raise AggregatePublicationConflict(
            'CSV buckets moved backwards across input scans: '
            f'source={bucket.key.source_id!r} bucket_start={bucket.key.bucket_start} '
            f'is older than already-published bucket {previous_start}. '
            'Rename or configure split inputs so their stable path order is chronological.'
        )
    reject_overlapping_canonical_bucket(
        conn,
        bucket,
        allowed_csv_scans={
            (
                event.scan_locator,
                input_revision.content_fingerprint,
                input_revision.decoder_fingerprint,
            )
        },
    )
    reject_incomplete_persisted_aggregate(conn, bucket, current_run_keys)
    bucket_revision = revision_for_locator(input_revision, event.input_locator)
    already_processed = conn.execute(
        """
        SELECT 1
        FROM processed_inputs
        WHERE input_kind = 'csv'
          AND input_locator = ?
          AND scan_locator = ?
          AND source_id = ?
          AND bucket_start = ?
          AND status = 'processed'
          AND revision_fingerprint = ?
        """,
        (
            event.input_locator,
            event.scan_locator,
            bucket.key.source_id,
            bucket.key.bucket_start,
            bucket_revision.fingerprint,
        ),
    ).fetchone()
    if already_processed is not None:
        return

    rows = canonical_bucket_rows(bucket)
    processed_bucket = {
        'input_kind': 'csv',
        'input_locator': event.input_locator,
        'scan_locator': event.scan_locator,
        'source_id': bucket.key.source_id,
        'bucket_start': bucket.key.bucket_start,
        'bucket_end': bucket.key.bucket_end,
        'input_revision': bucket_revision,
    }
    payload = {
        'processed_buckets': [processed_bucket],
        'traffic_rows': rows['traffic_rows'],
        'protocol_rows': rows['protocol_rows'],
        'address_count_rows': rows['address_count_rows'],
        'port_count_rows': rows['port_count_rows'],
        'address_structure_rows': build_address_structure_rows_from_raw_buckets(
            [bucket],
            maad_bin,
            maad_backend,
            maad_workers,
        )
        if run_maad
        else [],
        'canonical_buckets': [bucket],
    }
    write_input_payload(conn, payload, mark_processed=False)
    processed_buckets.append(processed_bucket)
    add_raw_bucket_to_streaming_aggregates(aggregate_buckets, bucket)
    published_through[bucket.key.source_id] = bucket.key.bucket_start
    current_run_keys.add((bucket.key.source_id, bucket.key.bucket_start))
    ready_aggregate_keys = [
        key
        for key, aggregate in aggregate_buckets.items()
        if key[0] == bucket.key.source_id
        and aggregate.key.bucket_end <= bucket.key.bucket_start
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


def reject_incomplete_persisted_aggregate(
    conn: sqlite3.Connection,
    bucket: CanonicalBucket,
    current_run_keys: set[tuple[str, int]],
) -> None:
    """Reject reopening a coarse interval whose exact address union is unavailable."""
    day_start = floor_bucket_start(bucket.key.bucket_start, 86400)
    day_end = next_bucket_start(day_start, 86400)
    persisted_starts = {
        int(row[0])
        for row in conn.execute(
            """
            SELECT DISTINCT bucket_start
            FROM traffic_stats
            WHERE source_id = ?
              AND granularity = '5m'
              AND bucket_start >= ?
              AND bucket_start < ?
            """,
            (bucket.key.source_id, day_start, day_end),
        ).fetchall()
    }
    external_starts = sorted(
        bucket_start
        for bucket_start in persisted_starts
        if bucket_start != bucket.key.bucket_start
        and (bucket.key.source_id, bucket_start) not in current_run_keys
    )
    if not external_starts:
        return
    raise AggregatePublicationConflict(
        'Cannot reopen a persisted aggregate interval exactly: '
        f'source={bucket.key.source_id!r} bucket_start={bucket.key.bucket_start} '
        f'shares its local day with persisted 5m bucket {external_starts[0]} from another run. '
        'Persisted address counts and MAAD output do not retain the address identities '
        'required to rebuild exact unique-address rollups.'
    )


def reject_overlapping_canonical_bucket(
    conn: sqlite3.Connection,
    bucket: CanonicalBucket,
    *,
    allowed_csv_scans: set[tuple[str, str, str]] | None = None,
    allowed_input_owners: set[tuple[str, str, str]] | None = None,
    replaceable_input_kinds: set[str] | None = None,
) -> None:
    """Reject a canonical 5m bucket already claimed by another logical owner."""
    allowed_csv_scans = set() if allowed_csv_scans is None else allowed_csv_scans
    allowed_input_owners = set() if allowed_input_owners is None else allowed_input_owners
    replaceable_input_kinds = (
        set() if replaceable_input_kinds is None else replaceable_input_kinds
    )
    owners = conn.execute(
        """
        SELECT input_kind, input_locator, scan_locator,
               content_fingerprint, decoder_fingerprint, revision_fingerprint
        FROM processed_inputs
        WHERE source_id = ?
          AND bucket_start = ?
        ORDER BY input_kind, input_locator, scan_locator
        """,
        (bucket.key.source_id, bucket.key.bucket_start),
    ).fetchall()
    conflicting_owners = [
        (input_kind, input_locator, scan_locator)
        for input_kind, input_locator, scan_locator, content, decoder, revision in owners
        if not (
            (
                input_kind == 'csv'
                and (scan_locator, content, decoder) in allowed_csv_scans
            )
            or (input_kind, input_locator, revision) in allowed_input_owners
            or input_kind in replaceable_input_kinds
        )
    ]
    if not conflicting_owners:
        return
    owner_text = ', '.join(
        f'{input_kind}:{input_locator}'
        for input_kind, input_locator, _scan_locator in conflicting_owners
    )
    raise AggregatePublicationConflict(
        'Overlapping canonical 5m input is not allowed: '
        f'source={bucket.key.source_id!r} bucket_start={bucket.key.bucket_start} '
        f'conflicts with {owner_text}.'
    )


def complete_csv_scans(
    conn: sqlite3.Connection,
    processed_buckets: list[dict],
    completion_events: list[tuple[CsvScanComplete, InputRevision, FileSnapshot]],
) -> None:
    """Atomically mark remaining buckets and successful CSV scans complete."""
    completions_by_locator = {
        event.scan_locator: (event, revision, snapshot)
        for event, revision, snapshot in completion_events
    }
    with conn:
        mark_processed_buckets(conn, processed_buckets)
        for scan_locator, (event, revision, snapshot) in sorted(
            completions_by_locator.items()
        ):
            verify_file_snapshot(scan_locator, snapshot)
            complete_input_scan(
                conn,
                input_kind='csv',
                scan_locator=scan_locator,
                rejected_rows=event.rejected_rows,
                skipped_bad_column_count=event.skipped_bad_column_count,
                input_revision=revision,
                file_snapshot=snapshot,
            )


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
    port_count_rows = [
        row
        for payload in stats_payloads
        for row in payload['port_count_rows']
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
        insert_stats_payload(
            conn,
            {
                'traffic_rows': traffic_rows,
                'protocol_rows': protocol_rows,
                'address_count_rows': address_count_rows,
                'port_count_rows': port_count_rows,
                'address_structure_rows': address_structure_rows,
            },
        )


def delete_aggregate_outputs_for_streaming_buckets(
    conn: sqlite3.Connection,
    buckets: list[CanonicalBucket],
) -> None:
    """Delete stale aggregate rows for streaming aggregate buckets before rewrite."""
    for bucket in buckets:
        delete_stats_bucket_keys(
            conn,
            [(bucket.key.source_id, bucket.key.granularity, bucket.key.bucket_start)],
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
    input_revision = spec['input_revision']
    if input_kind == 'nfcapd':
        if spec.get('gap'):
            payload = build_nfcapd_gap_payload(
                input_locator=input_locator,
                source_id=str(spec['source_id']),
                bucket_start=int(spec['bucket_start']),
                run_maad=run_maad,
                input_revision=input_revision,
            )
            payload['absence_snapshots'] = [spec['_absence_snapshot']]
            for bucket in payload['processed_buckets']:
                bucket['_absence_snapshots'] = payload['absence_snapshots']
            return payload
        nfcapd_payload = build_nfcapd_bucket_payload(
            input_locator,
            str(spec['source_id']),
            spec.get('_flow_selection', FlowSelection()),
        )
        verify_file_snapshot(input_locator, spec['_file_snapshot'])
        nfcapd_payload['processed_bucket']['input_revision'] = input_revision
        nfcapd_payload['processed_bucket']['file_snapshot'] = spec['_file_snapshot']
        canonical_bucket = nfcapd_payload['canonical_bucket']
        return {
            'processed_buckets': [nfcapd_payload['processed_bucket']],
            'traffic_rows': nfcapd_payload['traffic_rows'],
            'protocol_rows': nfcapd_payload['protocol_rows'],
            'address_count_rows': nfcapd_payload['address_count_rows'],
            'port_count_rows': nfcapd_payload['port_count_rows'],
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

    raise ValueError(f'Unsupported worker input_kind: {input_kind}')


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
    for bucket in processed_buckets:
        snapshot = bucket.get('file_snapshot')
        if snapshot is not None:
            verify_file_snapshot(bucket['input_locator'], snapshot)
    for absence in payload.get('absence_snapshots', []):
        absence.verify()
    with conn:
        if delete_existing:
            delete_5m_outputs_for_processed_buckets(conn, processed_buckets)

        for bucket in processed_buckets:
            upsert_input_bucket(
                conn,
                **{key: value for key, value in bucket.items() if not key.startswith('_')},
            )

        insert_stats_payload(conn, payload)

        if mark_processed:
            mark_processed_buckets(conn, processed_buckets)
        for absence in payload.get('absence_snapshots', []):
            absence.verify()


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
    delete_stats_bucket_keys(conn, [(source_id, '5m', bucket_start)])


def mark_processed_buckets(conn: sqlite3.Connection, processed_buckets: list[dict]) -> None:
    """Mark input buckets processed after all outputs are written."""
    absences = [
        absence
        for bucket in processed_buckets
        for absence in bucket.get('_absence_snapshots', [])
    ]
    for absence in absences:
        absence.verify()
    for bucket in processed_buckets:
        mark_input_bucket_status(
            conn,
            input_kind=bucket['input_kind'],
            input_locator=bucket['input_locator'],
            source_id=bucket['source_id'],
            bucket_start=bucket['bucket_start'],
            status='processed',
            input_revision=bucket['input_revision'],
        )
    for absence in absences:
        absence.verify()


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
    port_count_rows = [
        row
        for payload in stats_payloads
        for row in payload['port_count_rows']
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
        insert_stats_payload(
            conn,
            {
                'traffic_rows': traffic_rows,
                'protocol_rows': protocol_rows,
                'address_count_rows': address_count_rows,
                'port_count_rows': port_count_rows,
                'address_structure_rows': address_structure_rows,
            },
        )


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
        delete_stats_bucket_keys(conn, [(source_id, granularity, bucket_start)])


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


def build_nfcapd_gap_payload(
    input_locator: str,
    source_id: str,
    bucket_start: int,
    *,
    run_maad: bool = True,
    input_revision: InputRevision | None = None,
) -> dict:
    """Build zero-valued canonical rows for one bounded missing nfcapd bucket."""
    bucket_end = bucket_start + FIVE_MINUTE_SECONDS
    if input_revision is None:
        input_revision = gap_input_revision('nfcapd', input_locator)
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
                'input_revision': input_revision,
            }
        ],
        'traffic_rows': rows['traffic_rows'],
        'protocol_rows': rows['protocol_rows'],
        'address_count_rows': rows['address_count_rows'],
        'port_count_rows': rows['port_count_rows'],
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
    parser.add_argument('--ip-prefix', help='Only ingest flows with either endpoint in this CIDR.')
    parser.add_argument(
        '--src-visibility',
        choices=('literal', 'anonymized'),
        help='Only ingest flows with this source-address visibility.',
    )
    parser.add_argument(
        '--dst-visibility',
        choices=('literal', 'anonymized'),
        help='Only ingest flows with this destination-address visibility.',
    )
    parser.add_argument('--maad-bin', help='Path to MAAD binary.')
    parser.add_argument('--max-workers', type=int, help='Worker process count.')
    parser.add_argument('--force', action='store_true', help='Rewrite selected nfcapd buckets even when marked processed.')
    args = parser.parse_args()

    if args.config:
        if args.ip_prefix or args.src_visibility or args.dst_visibility:
            parser.error(
                'flow selection must be defined by the top-level selection object in --config mode'
            )
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
            selection = FlowSelection.from_payload(
                {
                    'ip_prefix': args.ip_prefix,
                    'src_visibility': args.src_visibility,
                    'dst_visibility': args.dst_visibility,
                }
            )
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
                selection=selection,
            )
        except ValueError as error:
            parser.error(str(error))

    db_path = Path(config['database_path'])
    db_path.parent.mkdir(parents=True, exist_ok=True)

    with sqlite3.connect(db_path) as conn:
        process_pipeline_config(conn, config)


if __name__ == '__main__':
    main()
