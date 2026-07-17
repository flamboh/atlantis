"""CSV file and archive readers for pipeline."""

from __future__ import annotations

import csv
import io
import tarfile
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path
from typing import TextIO

from csv_ingest import CsvSourceConfig, CsvSourceConfigError, load_csv_source_config
from normalized_rows import NormalizedRow, normalize_csv_row, normalize_csv_values


@dataclass(frozen=True, slots=True)
class CsvValueRow:
    values: list[str]
    locator: str
    line_number: int


def iter_csv_rows(path: str | Path, mapping_path: str | Path) -> Iterable[NormalizedRow]:
    """Yield normalized rows from a configured CSV input."""
    config = load_csv_source_config(mapping_path)
    input_path = Path(path)
    if is_tar_archive(input_path):
        yield from iter_csv_archive_rows(input_path, config)
        return

    with open(input_path, 'r', encoding='utf-8', newline='') as handle:
        yield from iter_csv_handle(handle, config, locator=str(input_path))


def iter_csv_archive_rows(path: Path, config: CsvSourceConfig) -> Iterable[NormalizedRow]:
    """Yield normalized rows from configured CSV members in a tar archive."""
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
                yield from iter_csv_handle(text, config, locator=f'{path}:{member.name}')


def iter_headerless_csv_values(path: str | Path, config: CsvSourceConfig) -> Iterable[CsvValueRow]:
    """Yield validated headerless CSV values without normalizing each row."""
    input_path = Path(path)
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
                with extracted, io.TextIOWrapper(extracted, encoding='utf-8', errors='replace') as text:
                    yield from iter_headerless_csv_handle_values(
                        text,
                        config,
                        locator=f'{input_path}:{member.name}',
                    )
        return

    with open(input_path, 'r', encoding='utf-8', newline='') as handle:
        yield from iter_headerless_csv_handle_values(handle, config, locator=str(input_path))


def iter_csv_handle(
    handle: TextIO,
    config: CsvSourceConfig,
    *,
    locator: str = '<csv>',
) -> Iterable[NormalizedRow]:
    """Yield normalized rows from an opened configured CSV stream."""
    if config.has_header:
        reader = csv.DictReader(handle, delimiter=config.delimiter)
        validate_header_columns(reader.fieldnames, config, locator)
        for line_number, row in enumerate(reader, start=2):
            if is_blank_row(row.values()):
                continue
            yield normalize_csv_row_with_context(row, config, locator, line_number)
        return

    if config.fieldnames is None:
        raise CsvSourceConfigError('CSV config fieldnames are required when has_header is false.')

    field_indexes = build_field_indexes(config, locator)
    uses_simple_split = config.delimiter == ','
    if uses_simple_split:
        rows = iter_simple_delimited_values(handle, config.delimiter)
    else:
        rows = csv.reader(handle, delimiter=config.delimiter)

    for line_number, values in enumerate(rows, start=1):
        if values == [''] or (not uses_simple_split and is_blank_row(values)):
            continue
        if len(values) != len(config.fieldnames):
            if config.skip_bad_column_count:
                continue
            raise CsvSourceConfigError(
                f'{locator}:{line_number}: CSV row must contain '
                f'{len(config.fieldnames)} values, got {len(values)}.'
            )
        yield normalize_csv_values_with_context(values, config, field_indexes, locator, line_number)


def iter_headerless_csv_handle_values(
    handle: TextIO,
    config: CsvSourceConfig,
    *,
    locator: str,
) -> Iterable[CsvValueRow]:
    """Yield validated field values for a headerless CSV stream."""
    if config.has_header:
        raise CsvSourceConfigError('Fast CSV value iteration requires has_header=false.')
    if config.fieldnames is None:
        raise CsvSourceConfigError('CSV config fieldnames are required when has_header is false.')

    build_field_indexes(config, locator)
    uses_simple_split = config.delimiter == ','
    if uses_simple_split:
        rows = iter_simple_delimited_values(handle, config.delimiter)
    else:
        rows = csv.reader(handle, delimiter=config.delimiter)

    for line_number, values in enumerate(rows, start=1):
        if values == [''] or (not uses_simple_split and is_blank_row(values)):
            continue
        if len(values) != len(config.fieldnames):
            if config.skip_bad_column_count:
                continue
            raise CsvSourceConfigError(
                f'{locator}:{line_number}: CSV row must contain '
                f'{len(config.fieldnames)} values, got {len(values)}.'
            )
        yield CsvValueRow(values=values, locator=locator, line_number=line_number)


def iter_simple_delimited_values(handle: TextIO, delimiter: str) -> Iterable[list[str]]:
    """Yield unquoted delimiter-split values for high-volume headerless flow CSV."""
    for line in handle:
        yield line.rstrip('\r\n').split(delimiter)


def is_blank_row(values) -> bool:
    """Return true for empty/whitespace-only CSV rows."""
    return all(value is None or str(value).strip() == '' for value in values)


def validate_header_columns(
    fieldnames: list[str] | None,
    config: CsvSourceConfig,
    locator: str,
) -> None:
    """Fail before row iteration when a header CSV lacks mapped columns."""
    if fieldnames is None:
        raise CsvSourceConfigError(f'{locator}: CSV header row is missing.')
    headers = set(fieldnames)
    required = set(config.columns.values())
    if config.source_id_column is not None:
        required.add(config.source_id_column)
    missing = sorted(required - headers)
    if missing:
        raise CsvSourceConfigError(
            f'{locator}: CSV header is missing mapped columns: {", ".join(missing)}.'
        )


def build_field_indexes(config: CsvSourceConfig, locator: str) -> dict[str, int]:
    """Precompute headerless field indexes used by the normalizer hot path."""
    assert config.fieldnames is not None
    indexes = {field_name: index for index, field_name in enumerate(config.fieldnames)}
    required = set(config.columns.values())
    if config.source_id_column is not None:
        required.add(config.source_id_column)
    missing = sorted(required - set(indexes))
    if missing:
        raise CsvSourceConfigError(
            f'{locator}: CSV fieldnames are missing mapped columns: {", ".join(missing)}.'
        )
    return indexes


def normalize_csv_row_with_context(
    row: dict,
    config: CsvSourceConfig,
    locator: str,
    line_number: int,
) -> NormalizedRow:
    """Normalize a row while preserving file/member/line diagnostics."""
    try:
        return normalize_csv_row(row, config)
    except CsvSourceConfigError as error:
        raise CsvSourceConfigError(f'{locator}:{line_number}: {error}') from error


def normalize_csv_values_with_context(
    values: list[str],
    config: CsvSourceConfig,
    field_indexes: dict[str, int],
    locator: str,
    line_number: int,
) -> NormalizedRow:
    """Normalize indexed values while preserving file/member/line diagnostics."""
    try:
        return normalize_csv_values(values, config, field_indexes)
    except CsvSourceConfigError as error:
        raise CsvSourceConfigError(f'{locator}:{line_number}: {error}') from error


def discover_csv_specs(
    root_path: str | Path,
    mapping_path: str | Path,
    config: CsvSourceConfig | None = None,
) -> list[dict]:
    """Discover configured CSV inputs under a flat root directory."""
    root = Path(root_path)
    config = load_csv_source_config(mapping_path) if config is None else config
    specs = []
    for path in sorted(root.iterdir()):
        if not path.is_file():
            continue
        if has_incomplete_download_sidecar(path):
            continue
        name = path.name.lower()
        if any(name.endswith(suffix) for suffix in config.discovery_exclude_suffixes):
            continue
        if matches_csv_discovery(name, config):
            specs.append(
                {
                    'input_kind': 'csv',
                    'path': str(path),
                    'mapping_path': str(mapping_path),
                }
            )
    return sorted(specs, key=lambda spec: csv_discovery_sort_key(Path(spec['path']).name))


def matches_csv_discovery(name: str, config: CsvSourceConfig) -> bool:
    """Return true when a filename matches configured CSV discovery rules."""
    if any(name.endswith(suffix) for suffix in config.discovery_include_suffixes):
        return True
    return any(fragment in name for fragment in config.discovery_include_contains)


def has_incomplete_download_sidecar(path: Path) -> bool:
    """Return true when a downloader sidecar marks this file incomplete."""
    return path.with_name(f'{path.name}.aria2').exists()


def csv_discovery_sort_key(name: str) -> tuple[int, int, str]:
    """Sort month/week named CSV datasets chronologically when possible."""
    normalized = name.lower().replace('.', '_').replace('-', '_')
    parts = normalized.split('_')
    month_order = {
        'january': 1,
        'february': 2,
        'march': 3,
        'april': 4,
        'may': 5,
        'june': 6,
        'july': 7,
        'august': 8,
        'september': 9,
        'october': 10,
        'november': 11,
        'december': 12,
    }
    month = next((month_order[part] for part in parts if part in month_order), 99)
    week = 99
    for part in parts:
        if part.startswith('week'):
            raw_week = part.removeprefix('week')
            if raw_week.isdigit():
                week = int(raw_week)
                break
    return (month, week, name)


def is_tar_archive(path: Path) -> bool:
    """Return true for compressed tar archives accepted by the CSV reader."""
    name = path.name.lower()
    return name.endswith('.tar.gz') or name.endswith('.tgz')
