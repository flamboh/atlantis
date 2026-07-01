"""Core extraction workflow for portable NetFlow analysis windows."""

from __future__ import annotations

import argparse
import shutil
import sqlite3
import tempfile
from collections.abc import Callable, Sequence
from contextlib import closing, nullcontext
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .config import (
    DEFAULT_DATASET_ID,
    DEFAULT_END_EXCLUSIVE,
    DEFAULT_TIMEZONE,
    MANIFEST_FILENAME,
    SQLITE_FILENAME,
    TABLE_CONFIG,
    TableConfig,
    TableManifest,
    compute_window,
    output_modes,
    resolve_default_source_db,
    resolve_parquet_dir,
    resolve_path,
    selected_outputs,
)
from .config import quote_identifier
from .manifest import build_manifest, write_manifest
from .parquet import export_table_to_parquet, parquet_modules
from .publish import publish_outputs
from .sqlite import (
    build_filters,
    connect_db,
    create_source_snapshot,
    create_sqlite_table,
    copy_table_to_sqlite,
    managed_output_paths,
    validate_parquet_dir,
    validate_required_tables,
    validate_source_db_managed_files,
)


@dataclass(frozen=True)
class ExtractionContext:
    start_ts: int
    end_ts: int
    source_id: str | None
    granularities: list[str] | None
    batch_size: int
    temp_parquet_dir: Path | None
    parquet_dir: Path | None


@dataclass(frozen=True)
class ExtractionHelpers:
    build_filters: Callable[..., tuple[str, tuple[Any, ...]]] = build_filters
    create_source_snapshot: Callable[[Path, Path], None] = create_source_snapshot
    create_sqlite_table: Callable[[sqlite3.Connection, sqlite3.Connection, str], None] = create_sqlite_table
    copy_table_to_sqlite: Callable[[sqlite3.Connection, sqlite3.Connection, str, str, tuple[Any, ...], int], int] = (
        copy_table_to_sqlite
    )
    export_table_to_parquet: Callable[[sqlite3.Connection, Path, str, str, tuple[Any, ...], int], int] = (
        export_table_to_parquet
    )
    publish_outputs: Callable[..., None] = publish_outputs


def resolved_paths(args: argparse.Namespace) -> tuple[Path, Path, Path | None, Path | None, Path]:
    source_db = Path(args.source_db).expanduser().resolve()
    output_dir = resolve_path(args.output_dir)
    sqlite_output_path = output_dir / SQLITE_FILENAME if "sqlite" in args.outputs else None
    parquet_dir = resolve_parquet_dir(args, output_dir, set(args.outputs))
    manifest_path = output_dir / MANIFEST_FILENAME
    return source_db, output_dir, sqlite_output_path, parquet_dir, manifest_path


def normalized_granularities(value: Any) -> list[str] | None:
    if value is None:
        return None
    return list(value)


def optional_connection(path: Path | None):
    if path is None:
        return nullcontext(None)
    return closing(connect_db(path))


def collect_table_summary(
    conn: sqlite3.Connection,
    table: str,
    config: TableConfig,
    where_sql: str,
    params: tuple[Any, ...],
) -> TableManifest:
    time_column = quote_identifier(config["time_column"])
    row = conn.execute(
        f"SELECT COUNT(*) AS row_count, MIN({time_column}) AS min_time, MAX({time_column}) AS max_time "
        f"FROM {quote_identifier(table)} {where_sql}",
        params,
    ).fetchone()
    return {
        "source_row_count": int(row["row_count"]),
        "source_min_time": row["min_time"],
        "source_max_time": row["max_time"],
    }


def print_dry_run(
    *,
    source_db: Path,
    output_dir: Path,
    sqlite_output_path: Path | None,
    parquet_dir: Path | None,
    start: str,
    end: str,
    tables: Sequence[str],
) -> None:
    print(f"[extract] source_db={source_db}")
    print(f"[extract] output_dir={output_dir}")
    print(f"[extract] sqlite_path={sqlite_output_path}")
    print(f"[extract] parquet_dir={parquet_dir}")
    print(f"[extract] window={start}..{end}")
    print(f"[extract] tables={', '.join(tables)}")


def print_table_summary(table: str, manifest: TableManifest) -> None:
    message = f"[extract] {table:<24} source={manifest['source_row_count']:>10}"
    if "sqlite_row_count" in manifest:
        message += f"  sqlite={manifest['sqlite_row_count']:>10}"
    if "parquet_row_count" in manifest:
        message += f"  parquet={manifest['parquet_row_count']:>10}"
    print(message)


def extract_table(
    source_conn: sqlite3.Connection,
    dest_conn: sqlite3.Connection | None,
    table: str,
    config: TableConfig,
    extraction: ExtractionContext,
    helpers: ExtractionHelpers | None = None,
) -> TableManifest:
    active_helpers = ExtractionHelpers() if helpers is None else helpers
    where_sql, params = active_helpers.build_filters(
        config=config,
        start_ts=extraction.start_ts,
        end_ts=extraction.end_ts,
        source_id=extraction.source_id,
        granularities=extraction.granularities,
    )
    manifest = collect_table_summary(source_conn, table, config, where_sql, params)
    if dest_conn is not None:
        active_helpers.create_sqlite_table(source_conn, dest_conn, table)
        manifest["sqlite_row_count"] = active_helpers.copy_table_to_sqlite(
            source_conn,
            dest_conn,
            table,
            where_sql,
            params,
            extraction.batch_size,
        )
    if extraction.temp_parquet_dir is not None and extraction.parquet_dir is not None:
        parquet_path = extraction.temp_parquet_dir / f"{table}.parquet"
        manifest["parquet_row_count"] = active_helpers.export_table_to_parquet(
            source_conn,
            parquet_path,
            table,
            where_sql,
            params,
            extraction.batch_size,
        )
        manifest["parquet_path"] = str(extraction.parquet_dir / f"{table}.parquet")
    print_table_summary(table, manifest)
    return manifest


def extract(args: argparse.Namespace, helpers: ExtractionHelpers | None = None) -> Path | None:
    active_helpers = ExtractionHelpers() if helpers is None else helpers
    args.outputs = selected_outputs(args)
    outputs = output_modes(args)
    source_db, output_dir, sqlite_output_path, parquet_dir, manifest_path = resolved_paths(args)
    if not source_db.exists():
        raise SystemExit(f"Source DB not found: {source_db}")
    managed_paths = managed_output_paths(output_dir, manifest_path)
    validate_source_db_managed_files(source_db, managed_paths)
    validate_parquet_dir(parquet_dir, source_db, managed_paths)
    start_dt, end_dt, start_ts, end_ts = compute_window(args.start, args.end_exclusive, args.timezone)
    dataset_id = str(getattr(args, "dataset_id", getattr(args, "dataset", DEFAULT_DATASET_ID)))
    source_id = getattr(args, "source_id", None)
    timezone = str(getattr(args, "timezone", DEFAULT_TIMEZONE))
    granularities = normalized_granularities(getattr(args, "granularity", None))
    batch_size = int(getattr(args, "batch_size", 5000))
    if batch_size < 1:
        raise SystemExit("--batch-size must be positive.")

    with closing(connect_db(source_db)) as source_conn:
        validate_required_tables(source_conn, TABLE_CONFIG)
    print_plan(
        args=args,
        source_db=source_db,
        output_dir=output_dir,
        sqlite_output_path=sqlite_output_path,
        parquet_dir=parquet_dir,
        manifest_path=manifest_path,
        start_dt=start_dt,
        end_dt=end_dt,
        start_ts=start_ts,
        end_ts=end_ts,
    )
    if args.dry_run:
        print("[extract] Dry run complete; no files written.")
        return None
    if "parquet" in outputs:
        parquet_modules()

    output_dir.mkdir(parents=True, exist_ok=True)

    work_dir = Path(tempfile.mkdtemp(prefix=".extract-window-", dir=output_dir))
    snapshot_path = work_dir / "source-snapshot.sqlite"
    temp_sqlite_path = None if sqlite_output_path is None else work_dir / SQLITE_FILENAME
    temp_parquet_dir = None if parquet_dir is None else work_dir / "parquet"
    if temp_parquet_dir is not None:
        temp_parquet_dir.mkdir()

    extraction = ExtractionContext(
        start_ts=start_ts,
        end_ts=end_ts,
        source_id=source_id,
        granularities=granularities,
        batch_size=batch_size,
        temp_parquet_dir=temp_parquet_dir,
        parquet_dir=parquet_dir,
    )

    try:
        active_helpers.create_source_snapshot(source_db, snapshot_path)
        table_manifests: dict[str, TableManifest] = {}
        with closing(connect_db(snapshot_path)) as source_conn:
            with optional_connection(temp_sqlite_path) as dest_conn:
                for table, table_config_value in TABLE_CONFIG.items():
                    table_manifests[table] = extract_table(
                        source_conn,
                        dest_conn,
                        table,
                        table_config_value,
                        extraction,
                        active_helpers,
                    )

        manifest = build_manifest(
            dataset_id=dataset_id,
            source_db=source_db,
            output_dir=output_dir,
            sqlite_output_path=sqlite_output_path,
            parquet_dir=parquet_dir,
            start=args.start,
            end_exclusive=args.end_exclusive,
            start_dt=start_dt,
            end_dt=end_dt,
            start_ts=start_ts,
            end_ts=end_ts,
            timezone=timezone,
            source_id=source_id,
            granularities=granularities,
            tables=table_manifests,
        )
        temp_manifest_path = write_manifest(work_dir, manifest)
        active_helpers.publish_outputs(
            temp_sqlite_path=temp_sqlite_path,
            sqlite_output_path=sqlite_output_path,
            temp_parquet_dir=temp_parquet_dir,
            parquet_dir=parquet_dir,
            temp_manifest_path=temp_manifest_path,
            manifest_path=manifest_path,
            work_dir=work_dir,
        )
    finally:
        if work_dir.exists():
            shutil.rmtree(work_dir)

    print_final_summary(
        manifest_path=manifest_path,
        sqlite_output_path=sqlite_output_path,
        parquet_dir=parquet_dir,
    )
    return manifest_path


def describe_table_filter(
    config: TableConfig,
    args: argparse.Namespace,
    start_ts: int,
    end_ts: int,
) -> str:
    filters = [f"{config['time_column']} >= {start_ts} and < {end_ts}"]
    if args.source_id is not None:
        filters.append(f"{config['source_column']} = {args.source_id}")
    if args.granularity is not None:
        filters.append(f"{config['granularity_column']} in {', '.join(args.granularity)}")
    return "; ".join(filters)


def print_plan(
    *,
    args: argparse.Namespace,
    source_db: Path,
    output_dir: Path,
    sqlite_output_path: Path | None,
    parquet_dir: Path | None,
    manifest_path: Path,
    start_dt: Any,
    end_dt: Any,
    start_ts: int,
    end_ts: int,
) -> None:
    granularity_filter = "all" if args.granularity is None else ", ".join(args.granularity)
    print("[extract] Plan")
    print(f"  source DB:   {source_db}")
    print(f"  output dir:  {output_dir}")
    print(f"  outputs:     {', '.join(args.outputs)}")
    print(f"  sqlite:      {sqlite_output_path if sqlite_output_path is not None else 'disabled'}")
    print(f"  parquet:     {parquet_dir if parquet_dir is not None else 'disabled'}")
    print(f"  manifest:    {manifest_path}")
    print(
        "  window:      "
        f"{start_dt.isoformat()} ({start_ts}) to {end_dt.isoformat()} ({end_ts}) exclusive"
    )
    print(f"  filters:     source_id={args.source_id or 'all'}, granularity={granularity_filter}")
    print("  tables:")
    for table, table_config_value in TABLE_CONFIG.items():
        print(f"    {table:<24} {describe_table_filter(table_config_value, args, start_ts, end_ts)}")


def print_final_summary(
    *,
    manifest_path: Path,
    sqlite_output_path: Path | None,
    parquet_dir: Path | None,
) -> None:
    print("[extract] Artifacts")
    if sqlite_output_path is not None:
        print(f"  sqlite:   {sqlite_output_path}")
    if parquet_dir is not None:
        print(f"  parquet:  {parquet_dir}")
    print(f"  manifest: {manifest_path}")
