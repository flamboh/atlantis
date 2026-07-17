"""Deep CSV scan module with provenance, coverage, and row rejection policy."""

from __future__ import annotations

import csv
import io
import logging
import os
import tarfile
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Iterator, Mapping
from urllib.parse import quote

from csv_ingest import (
    TIMESTAMP_KEYS,
    CsvSourceConfig,
    CsvSourceConfigError,
    load_csv_source_config,
    parse_timestamp,
    resolve_bucket_start,
    resolve_source_id,
)
from csv_inputs import build_field_indexes, is_blank_row, is_tar_archive, validate_header_columns
from normalized_rows import (
    NormalizedRow,
    normalize_csv_row,
    normalize_csv_values,
    resolve_source_id_from_values,
)
from statistical_bucket import BucketKey, CanonicalBucket, StatisticalBucket


LOGGER = logging.getLogger(__name__)
CSV_ARROW_BLOCK_BYTES = int(os.environ.get('CSV_ARROW_BLOCK_BYTES', str(64 * 1024 * 1024)))


@dataclass(frozen=True)
class CsvBucketReady:
    scan_locator: str
    input_locator: str
    bucket: CanonicalBucket


@dataclass(frozen=True)
class CsvScanComplete:
    scan_locator: str
    rejected_rows: int
    skipped_bad_column_count: int
    observed_bounds: dict[str, tuple[int, int]]


CsvScanEvent = CsvBucketReady | CsvScanComplete


@dataclass(frozen=True)
class _RawRow:
    values: Mapping[str, str | None] | None
    locator: str
    line_number: int
    indexed_values: list[str] | None = None


class _ScanState:
    def __init__(self, scan_locator: str, config: CsvSourceConfig) -> None:
        self.scan_locator = scan_locator
        self.config = config
        self.field_indexes = (
            build_field_indexes(config, scan_locator)
            if not config.has_header
            else None
        )
        self.buckets: dict[tuple[str, int], StatisticalBucket] = {}
        self.bounds: dict[str, tuple[int, int]] = {}
        self.next_emit: dict[str, int] = {}
        self.has_emitted: set[str] = set()
        self.rejected_rows = 0
        self.skipped_bad_column_count = 0

    def observe(self, row: Mapping[str, str | None]) -> tuple[str, int] | None:
        try:
            source_id = resolve_source_id(row, self.config)
            bucket_start = resolve_bucket_start(row, self.config)
        except CsvSourceConfigError:
            return None
        return self.record_observation(source_id, bucket_start)

    def observe_values(self, values: list[str]) -> tuple[str, int] | None:
        """Observe indexed coverage without constructing a row mapping."""
        assert self.field_indexes is not None
        try:
            source_id = resolve_source_id_from_values(values, self.config, self.field_indexes)
            bucket_start = _resolve_indexed_coverage_bucket(
                values,
                self.config,
                self.field_indexes,
            )
        except CsvSourceConfigError:
            return None
        return self.record_observation(source_id, bucket_start)

    def record_observation(self, source_id: str, bucket_start: int) -> tuple[str, int]:
        lower, upper = self.bounds.get(source_id, (bucket_start, bucket_start))
        self.bounds[source_id] = (min(lower, bucket_start), max(upper, bucket_start))
        if source_id not in self.has_emitted:
            self.next_emit[source_id] = min(self.next_emit.get(source_id, bucket_start), bucket_start)
        return source_id, bucket_start

    def accept(self, row: NormalizedRow) -> None:
        key = (row.source_id, row.bucket_start)
        bucket = self.buckets.setdefault(
            key,
            StatisticalBucket(
                BucketKey(row.source_id, '5m', row.bucket_start, row.bucket_end),
                dense=True,
            ),
        )
        bucket.add(row.observation)

    def reject(self, raw: _RawRow, error: CsvSourceConfigError) -> None:
        self.rejected_rows += 1
        LOGGER.warning('Rejected CSV row %s:%s: %s', raw.locator, raw.line_number, error)

    def emit_through(self, source_id: str, last_start: int) -> Iterator[CsvBucketReady]:
        next_start = self.next_emit[source_id]
        while next_start <= last_start:
            builder = self.buckets.pop((source_id, next_start), None)
            if builder is None:
                builder = StatisticalBucket(
                    BucketKey(source_id, '5m', next_start, next_start + 300),
                    dense=True,
                )
                input_locator = csv_gap_locator(self.scan_locator, source_id, next_start)
            else:
                input_locator = self.scan_locator
            yield CsvBucketReady(self.scan_locator, input_locator, builder.finish())
            next_start += 300
            self.has_emitted.add(source_id)
        self.next_emit[source_id] = next_start


def scan_csv(
    spec: Mapping[str, object],
    config: CsvSourceConfig,
) -> Iterable[CsvScanEvent]:
    """Scan one CSV file/archive and emit dense canonical buckets plus terminal completion."""
    scan_locator = str(spec['path'])
    if _should_use_arrow(config):
        yield from _scan_csv_arrow(Path(scan_locator), config)
        return
    state = _ScanState(scan_locator, config)

    for raw in _iter_raw_rows(Path(scan_locator), config, state):
        observed = (
            state.observe_values(raw.indexed_values)
            if raw.indexed_values is not None
            else state.observe(_require_mapping(raw.values))
        )
        try:
            normalized = (
                normalize_csv_values(
                    raw.indexed_values,
                    config,
                    state.field_indexes,
                )
                if raw.indexed_values is not None
                else normalize_csv_row(_require_mapping(raw.values), config)
            )
        except CsvSourceConfigError as error:
            state.reject(raw, error)
        else:
            state.accept(normalized)

        if config.input_order == 'timestamp_ascending' and observed is not None:
            source_id, bucket_start = observed
            cutoff = state.bounds[source_id][1] - (config.out_of_order_lag_buckets * 300)
            if bucket_start < state.next_emit[source_id]:
                raise ValueError(
                    f'CSV input is not ordered enough for streaming: {scan_locator} row bucket '
                    f'{bucket_start} arrived after flush cutoff. Set "input_order": "unsorted" '
                    'to use full-file aggregation.'
                )
            yield from state.emit_through(source_id, cutoff - 300)

    for source_id, (_lower, upper) in sorted(state.bounds.items()):
        yield from state.emit_through(source_id, upper)
    yield CsvScanComplete(
        scan_locator=scan_locator,
        rejected_rows=state.rejected_rows,
        skipped_bad_column_count=state.skipped_bad_column_count,
        observed_bounds=dict(sorted(state.bounds.items())),
    )


def csv_gap_locator(scan_locator: str, source_id: str, bucket_start: int) -> str:
    return (
        f'gap://csv/{quote(scan_locator, safe="")}/'
        f'{quote(source_id, safe="")}/{bucket_start}'
    )


def _require_mapping(values: Mapping[str, str | None] | None) -> Mapping[str, str | None]:
    assert values is not None
    return values


def _resolve_indexed_coverage_bucket(
    values: list[str],
    config: CsvSourceConfig,
    field_indexes: Mapping[str, int],
) -> int:
    """Match resolve_bucket_start precedence without allocating a row mapping."""
    for logical_key in TIMESTAMP_KEYS:
        column_name = config.columns.get(logical_key)
        if column_name is None:
            continue
        raw_value = values[field_indexes[column_name]]
        if raw_value in (None, ''):
            continue
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


def _iter_raw_rows(
    path: Path,
    config: CsvSourceConfig,
    state: _ScanState,
) -> Iterator[_RawRow]:
    if is_tar_archive(path):
        with tarfile.open(path, mode='r:*') as archive:
            for member in archive:
                if not member.isfile():
                    continue
                if config.archive_member_contains and config.archive_member_contains not in member.name:
                    continue
                extracted = archive.extractfile(member)
                if extracted is None:
                    continue
                with extracted, io.TextIOWrapper(extracted, encoding='utf-8', errors='replace') as text:
                    yield from _iter_handle_rows(
                        text,
                        config,
                        state,
                        locator=f'{path}:{member.name}',
                    )
        return

    with open(path, 'r', encoding='utf-8', newline='') as handle:
        yield from _iter_handle_rows(handle, config, state, locator=str(path))


def _should_use_arrow(config: CsvSourceConfig) -> bool:
    return (
        not config.has_header
        and config.fieldnames is not None
        and config.delimiter == ','
        and config.source_id_value is not None
        and config.timestamp_format == 'datetime'
        and config.datetime_format == '%Y-%m-%d %H:%M:%S'
    )


def _scan_csv_arrow(path: Path, config: CsvSourceConfig) -> Iterator[CsvScanEvent]:
    """Vectorize validation and filtering while preserving the shared scan contract."""
    import pyarrow as pa
    import pyarrow.compute as pc

    state = _ScanState(str(path), config)
    timestamp_columns = [config.columns[key] for key in TIMESTAMP_KEYS if key in config.columns]
    source_id = config.source_id_value
    assert source_id is not None

    for batch, locator in _iter_arrow_batches(path, config, state):
        columns = {
            name: pc.utf8_trim_whitespace(batch[name])
            for name in batch.schema.names
        }
        batch = pa.record_batch(columns)
        selected_timestamp = pa.array([''] * batch.num_rows)
        timestamp_mask = pa.array([True] * batch.num_rows)
        timestamp_buckets: dict[str, int] = {}
        for timestamp_column in reversed(timestamp_columns):
            values = batch[timestamp_column]
            selected_timestamp = pc.if_else(
                pc.not_equal(values, ''),
                values,
                selected_timestamp,
            )
            shaped = pc.match_substring_regex(
                values,
                r'^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$',
            )
            valid_values = []
            for raw_timestamp in pc.unique(batch.filter(shaped)[timestamp_column]).to_pylist():
                try:
                    timestamp_buckets[raw_timestamp] = resolve_bucket_start(
                        {timestamp_column: raw_timestamp}, config
                    )
                    valid_values.append(raw_timestamp)
                except CsvSourceConfigError:
                    continue
            column_valid = pc.or_(
                pc.equal(values, ''),
                pc.is_in(values, value_set=pa.array(valid_values, type=pa.string())),
            )
            timestamp_mask = pc.and_(timestamp_mask, column_valid)
        selected_valid = pc.and_(
            pc.not_equal(selected_timestamp, ''),
            pc.is_in(
                selected_timestamp,
                value_set=pa.array(list(timestamp_buckets), type=pa.string()),
            ),
        )
        timestamp_mask = pc.and_(timestamp_mask, selected_valid)
        selected_coverage_values = pc.unique(
            selected_timestamp.filter(selected_valid)
        ).to_pylist()
        for raw_timestamp in selected_coverage_values:
            bucket_start = timestamp_buckets[raw_timestamp]
            if source_id in state.has_emitted and bucket_start < state.next_emit[source_id]:
                raise ValueError(
                    f'CSV input is not ordered enough for streaming: {path} row bucket '
                    f'{bucket_start} arrived after flush cutoff. Set "input_order": "unsorted" '
                    'to use full-file aggregation.'
                )
            state.observe(_coverage_timestamp_row(config, bucket_start))

        mask = timestamp_mask
        src_column = config.columns['src_ip']
        dst_column = config.columns['dst_ip']
        src_ipv4 = pc.match_substring_regex(batch[src_column], _IPV4_REGEX)
        dst_ipv4 = pc.match_substring_regex(batch[dst_column], _IPV4_REGEX)
        src_ipv6 = pc.match_substring(batch[src_column], ':')
        dst_ipv6 = pc.match_substring(batch[dst_column], ':')
        mask = pc.and_(mask, pc.or_(pc.and_(src_ipv4, dst_ipv4), pc.and_(src_ipv6, dst_ipv6)))

        protocol_column = config.columns.get('protocol')
        if protocol_column is not None:
            protocol_text = batch[protocol_column]
            numeric = pc.match_substring_regex(protocol_text, r'^\d{1,3}$')
            valid_protocol_names = sorted(
                name
                for name, number in config.protocol_map.items()
                if 0 <= number <= 255
            )
            known = pc.is_in(
                pc.utf8_upper(protocol_text),
                value_set=pa.array(valid_protocol_names),
            )
            safe_protocol = pc.if_else(numeric, protocol_text, pa.scalar('0'))
            numeric_in_range = pc.and_(
                numeric,
                pc.less_equal(pc.cast(safe_protocol, pa.int16()), 255),
            )
            mask = pc.and_(
                mask,
                pc.or_(pc.equal(protocol_text, ''), pc.or_(numeric_in_range, known)),
            )
        for logical_key in ('packets', 'bytes'):
            column = config.columns.get(logical_key)
            if column is not None:
                mask = pc.and_(mask, pc.match_substring_regex(batch[column], r'^\d*$'))
        for logical_key in ('src_tos', 'dst_tos'):
            column = config.columns.get(logical_key)
            if column is not None:
                text = batch[column]
                numeric_or_empty = pc.match_substring_regex(text, r'^\d{0,3}$')
                safe_number = pc.if_else(
                    numeric_or_empty,
                    pc.if_else(pc.equal(text, ''), pa.scalar('0'), text),
                    pa.scalar('0'),
                )
                in_range = pc.less_equal(pc.cast(safe_number, pa.int16()), 255)
                mask = pc.and_(mask, pc.and_(numeric_or_empty, in_range))

        filtered = batch.filter(mask)
        state.rejected_rows += batch.num_rows - filtered.num_rows
        for row_number, raw_values in enumerate(filtered.to_pylist(), start=1):
            raw = _RawRow(raw_values, locator, row_number)
            try:
                state.accept(normalize_csv_row(raw_values, config))
            except (CsvSourceConfigError, ValueError) as error:
                if not isinstance(error, CsvSourceConfigError):
                    error = CsvSourceConfigError(str(error))
                state.reject(raw, error)

        if config.input_order == 'timestamp_ascending' and source_id in state.bounds:
            cutoff = state.bounds[source_id][1] - (config.out_of_order_lag_buckets * 300)
            yield from state.emit_through(source_id, cutoff - 300)

    for observed_source, (_lower, upper) in sorted(state.bounds.items()):
        yield from state.emit_through(observed_source, upper)
    yield CsvScanComplete(
        scan_locator=str(path),
        rejected_rows=state.rejected_rows,
        skipped_bad_column_count=state.skipped_bad_column_count,
        observed_bounds=dict(sorted(state.bounds.items())),
    )


_IPV4_REGEX = (
    r'^(?:[0-9]{1,2}|0[0-9]{2}|1[0-9]{2}|2[0-4][0-9]|25[0-5])'
    r'(?:\.(?:[0-9]{1,2}|0[0-9]{2}|1[0-9]{2}|2[0-4][0-9]|25[0-5])){3}$'
)


def _coverage_timestamp_row(config: CsvSourceConfig, bucket_start: int) -> dict[str, str]:
    """Build the minimum mapped timestamp row needed by the shared observer."""
    from datetime import datetime
    from zoneinfo import ZoneInfo

    logical_key = next(key for key in ('time_received', 'time_end', 'time_start') if key in config.columns)
    column = config.columns[logical_key]
    value = datetime.fromtimestamp(bucket_start, ZoneInfo(config.timestamp_timezone)).strftime(
        config.datetime_format
    )
    return {column: value}


def _iter_arrow_batches(path: Path, config: CsvSourceConfig, state: _ScanState):
    import pyarrow as pa
    import pyarrow.csv as arrow_csv

    assert config.fieldnames is not None

    def invalid_row_handler(_row):
        if config.skip_bad_column_count:
            state.skipped_bad_column_count += 1
            return 'skip'
        return 'error'

    read_options = arrow_csv.ReadOptions(
        column_names=config.fieldnames,
        block_size=CSV_ARROW_BLOCK_BYTES,
    )
    parse_options = arrow_csv.ParseOptions(
        delimiter=config.delimiter,
        invalid_row_handler=invalid_row_handler,
    )
    convert_options = arrow_csv.ConvertOptions(
        column_types={field_name: pa.string() for field_name in config.fieldnames}
    )

    def read(handle, locator: str):
        try:
            reader = arrow_csv.open_csv(
                handle,
                read_options=read_options,
                parse_options=parse_options,
                convert_options=convert_options,
            )
        except pa.ArrowInvalid as error:
            raise CsvSourceConfigError(f'{locator}: {error}') from error
        try:
            while True:
                try:
                    yield reader.read_next_batch(), locator
                except StopIteration:
                    break
                except pa.ArrowInvalid as error:
                    raise CsvSourceConfigError(f'{locator}: {error}') from error
        finally:
            reader.close()

    if is_tar_archive(path):
        with tarfile.open(path, mode='r:*') as archive:
            for member in archive:
                if not member.isfile():
                    continue
                if config.archive_member_contains and config.archive_member_contains not in member.name:
                    continue
                extracted = archive.extractfile(member)
                if extracted is None:
                    continue
                with extracted:
                    yield from read(extracted, f'{path}:{member.name}')
        return
    with open(path, 'rb') as handle:
        yield from read(handle, str(path))


def _iter_handle_rows(handle, config: CsvSourceConfig, state: _ScanState, *, locator: str) -> Iterator[_RawRow]:
    if config.has_header:
        reader = csv.DictReader(handle, delimiter=config.delimiter)
        validate_header_columns(reader.fieldnames, config, locator)
        for line_number, row in enumerate(reader, start=2):
            if is_blank_row(row.values()):
                continue
            if None in row or any(value is None for value in row.values()):
                if config.skip_bad_column_count:
                    state.skipped_bad_column_count += 1
                    continue
                raise CsvSourceConfigError(
                    f'{locator}:{line_number}: CSV row does not match the header column count.'
                )
            yield _RawRow(row, locator, line_number)
        return

    assert config.fieldnames is not None
    reader = csv.reader(handle, delimiter=config.delimiter)
    for line_number, values in enumerate(reader, start=1):
        if is_blank_row(values):
            continue
        if len(values) != len(config.fieldnames):
            if config.skip_bad_column_count:
                state.skipped_bad_column_count += 1
                continue
            raise CsvSourceConfigError(
                f'{locator}:{line_number}: CSV row must contain '
                f'{len(config.fieldnames)} values, got {len(values)}.'
            )
        yield _RawRow(
            None,
            locator,
            line_number,
            indexed_values=values,
        )
