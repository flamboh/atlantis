"""SQLite helpers for NetFlow window extraction."""

from __future__ import annotations

import json
import sqlite3
from collections.abc import Iterator, Sequence
from contextlib import closing
from pathlib import Path
from typing import Any

from flow_selection import FlowSelection
from pipeline_product import ProductIdentity
from stats import STATS_TABLE_ADAPTERS

from .config import (
    REQUIRED_TABLE_COLUMNS,
    SQLITE_FILENAME,
    SQLITE_SIDECAR_SUFFIXES,
    TableConfig,
    quote_identifier,
)


def connect_db(path: Path) -> sqlite3.Connection:
    conn = sqlite3.connect(path)
    conn.row_factory = sqlite3.Row
    return conn


def sqlite_artifact_paths(path: Path) -> list[Path]:
    return [path, *(path.with_name(path.name + suffix) for suffix in SQLITE_SIDECAR_SUFFIXES)]


def managed_output_paths(output_dir: Path, manifest_path: Path) -> list[Path]:
    sqlite_path = output_dir / SQLITE_FILENAME
    return [manifest_path, *sqlite_artifact_paths(sqlite_path)]


def validate_required_tables(conn: sqlite3.Connection, table_config: dict[str, TableConfig]) -> None:
    existing = {
        str(row["name"])
        for row in conn.execute("SELECT name FROM sqlite_master WHERE type = 'table'")
    }
    missing = [table for table in table_config if table not in existing]
    if missing:
        raise SystemExit(f"Missing required source table(s): {', '.join(missing)}")

    missing_columns = []
    for table, config in table_config.items():
        actual_columns = get_table_column_types(conn, table)
        required_columns = set(config.values()) | set(REQUIRED_TABLE_COLUMNS[table])
        missing_columns.extend(
            f"{table}.{column}" for column in sorted(required_columns) if column not in actual_columns
        )
    if missing_columns:
        raise SystemExit(f"Missing required source column(s): {', '.join(missing_columns)}")


def read_pipeline_product(conn: sqlite3.Connection) -> dict[str, Any]:
    """Return a verified pipeline product contract for an analysis export."""
    required_columns = {
        'singleton',
        'schema_json',
        'schema_fingerprint',
        'selection_json',
        'selection_fingerprint',
        'config_json',
        'config_fingerprint',
        'product_fingerprint',
    }
    try:
        actual_columns = get_table_column_types(conn, 'pipeline_product')
    except RuntimeError as error:
        raise SystemExit('Missing required source table: pipeline_product') from error
    missing = sorted(required_columns - set(actual_columns))
    if missing:
        raise SystemExit(
            'Missing required source column(s): '
            + ', '.join(f'pipeline_product.{column}' for column in missing)
        )
    rows = conn.execute(
        """
        SELECT singleton, schema_json, schema_fingerprint,
               selection_json, selection_fingerprint,
               config_json, config_fingerprint, product_fingerprint
        FROM pipeline_product
        """
    ).fetchall()
    if len(rows) != 1 or rows[0]['singleton'] != 1:
        raise SystemExit('Source DB must contain exactly one pipeline_product singleton row.')
    row = rows[0]
    try:
        schema = json.loads(row['schema_json'])
        raw_selection = json.loads(row['selection_json'])
        config = json.loads(row['config_json'])
        selection = FlowSelection.from_payload(raw_selection).normalized_payload()
    except (json.JSONDecodeError, TypeError, ValueError) as error:
        raise SystemExit(f'Invalid source pipeline_product contract: {error}') from error

    expected_schema = {
        'version': 2,
        'tables': [
            {'name': adapter.table_name, 'version': adapter.schema_version}
            for adapter in STATS_TABLE_ADAPTERS
        ],
    }
    if schema != expected_schema:
        raise SystemExit(
            'Source pipeline_product uses an unsupported schema; '
            'export-window requires the current observation-metrics product schema.'
        )

    expected = ProductIdentity.create(schema=schema, selection=selection, config=config)
    stored_values = {
        'schema_json': row['schema_json'],
        'schema_fingerprint': row['schema_fingerprint'],
        'selection_json': row['selection_json'],
        'selection_fingerprint': row['selection_fingerprint'],
        'config_json': row['config_json'],
        'config_fingerprint': row['config_fingerprint'],
        'product_fingerprint': row['product_fingerprint'],
    }
    expected_values = {
        'schema_json': expected.schema_json,
        'schema_fingerprint': expected.schema_fingerprint,
        'selection_json': expected.selection_json,
        'selection_fingerprint': expected.selection_fingerprint,
        'config_json': expected.config_json,
        'config_fingerprint': expected.config_fingerprint,
        'product_fingerprint': expected.fingerprint,
    }
    mismatches = [name for name, value in stored_values.items() if value != expected_values[name]]
    if mismatches:
        raise SystemExit(
            'Source pipeline_product identity is internally inconsistent: '
            + ', '.join(mismatches)
        )
    return {
        'product_fingerprint': expected.fingerprint,
        'schema_fingerprint': expected.schema_fingerprint,
        'selection_fingerprint': expected.selection_fingerprint,
        'selection': selection,
    }


def validate_parquet_dir(
    parquet_dir: Path | None,
    source_db: Path,
    managed_file_paths: Sequence[Path],
) -> None:
    if parquet_dir is None:
        return

    if parquet_dir == source_db:
        raise SystemExit("--parquet-dir must differ from source DB.")
    if source_db.is_relative_to(parquet_dir):
        raise SystemExit("--parquet-dir must not contain source DB.")
    if parquet_dir.is_relative_to(source_db):
        raise SystemExit("--parquet-dir must not be inside source DB path.")

    for managed_path in managed_file_paths:
        if parquet_dir == managed_path:
            raise SystemExit(f"--parquet-dir must differ from managed output path: {managed_path}")
        if managed_path.is_relative_to(parquet_dir):
            raise SystemExit(f"--parquet-dir must not contain managed output path: {managed_path}")
        if parquet_dir.is_relative_to(managed_path):
            raise SystemExit(f"--parquet-dir must not be inside managed output path: {managed_path}")


def validate_source_db_managed_files(source_db: Path, managed_file_paths: Sequence[Path]) -> None:
    for managed_path in managed_file_paths:
        if source_db == managed_path:
            raise SystemExit(f"--source-db must differ from managed output path: {managed_path}")
        if source_db.is_relative_to(managed_path):
            raise SystemExit(f"--source-db must not be inside managed output path: {managed_path}")


def create_source_snapshot(source_db: Path, snapshot_path: Path) -> None:
    with closing(connect_db(source_db)) as source_conn, closing(connect_db(snapshot_path)) as snapshot_conn:
        source_conn.backup(snapshot_conn)


def get_table_column_types(conn: sqlite3.Connection, table: str) -> dict[str, str]:
    rows = conn.execute(f"PRAGMA table_info({quote_identifier(table)})").fetchall()
    if not rows:
        raise RuntimeError(f"Table not found or has no columns: {table}")
    return {str(row["name"]): str(row["type"]) for row in rows}


def get_schema_sql(conn: sqlite3.Connection, table: str) -> list[str]:
    rows = conn.execute(
        """
        SELECT type, name, sql
        FROM sqlite_master
        WHERE tbl_name = ?
          AND type IN ('table', 'index')
          AND sql IS NOT NULL
        ORDER BY CASE type WHEN 'table' THEN 0 ELSE 1 END, name
        """,
        (table,),
    ).fetchall()
    if not rows:
        raise RuntimeError(f"Missing schema for table: {table}")
    return [str(row["sql"]) for row in rows]


def create_sqlite_table(
    source_conn: sqlite3.Connection,
    dest_conn: sqlite3.Connection,
    table: str,
) -> None:
    for sql in get_schema_sql(source_conn, table):
        dest_conn.execute(sql)
    dest_conn.commit()


def build_filters(
    *,
    config: TableConfig,
    start_ts: int,
    end_ts: int,
    source_id: str | None,
    granularities: Sequence[str] | None,
) -> tuple[str, tuple[Any, ...]]:
    clauses = [
        f"{quote_identifier(config['time_column'])} >= ? "
        f"AND {quote_identifier(config['time_column'])} < ?"
    ]
    params: list[Any] = [start_ts, end_ts]

    if source_id is not None:
        clauses.append(f"{quote_identifier(config['source_column'])} = ?")
        params.append(source_id)

    if granularities is not None:
        placeholders = ", ".join("?" for _ in granularities)
        clauses.append(f"{quote_identifier(config['granularity_column'])} IN ({placeholders})")
        params.extend(granularities)

    return f"WHERE {' AND '.join(clauses)}", tuple(params)


def iter_table_batches(
    conn: sqlite3.Connection,
    table: str,
    where_sql: str,
    params: tuple[Any, ...],
    batch_size: int,
) -> Iterator[list[sqlite3.Row]]:
    cursor = conn.execute(f"SELECT * FROM {quote_identifier(table)} {where_sql}", params)
    while rows := cursor.fetchmany(batch_size):
        yield rows


def copy_table_to_sqlite(
    source_conn: sqlite3.Connection,
    dest_conn: sqlite3.Connection,
    table: str,
    where_sql: str,
    params: tuple[Any, ...],
    batch_size: int,
) -> int:
    columns = list(get_table_column_types(source_conn, table))
    column_sql = ", ".join(quote_identifier(column) for column in columns)
    placeholders = ", ".join("?" for _ in columns)
    insert_sql = f"INSERT INTO {quote_identifier(table)} ({column_sql}) VALUES ({placeholders})"
    inserted = 0

    for rows in iter_table_batches(source_conn, table, where_sql, params, batch_size):
        payload = [tuple(row[column] for column in columns) for row in rows]
        dest_conn.executemany(insert_sql, payload)
        inserted += len(payload)

    dest_conn.commit()
    return inserted
