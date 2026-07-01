"""Parquet export helpers for NetFlow window extraction."""

from __future__ import annotations

import sqlite3
from pathlib import Path
from typing import Any

from .config import quote_identifier
from .sqlite import get_table_column_types, iter_table_batches


def parquet_modules():
    try:
        import pyarrow as pa
        import pyarrow.parquet as pq
    except ModuleNotFoundError as error:
        raise SystemExit("Parquet export requires pyarrow in the active environment.") from error
    return pa, pq


def arrow_type_for(
    conn: sqlite3.Connection,
    table: str,
    column: str,
    declared_type: str,
    where_sql: str,
    params: tuple[Any, ...],
    pa: Any,
) -> Any:
    normalized = declared_type.upper()
    if "INT" in normalized:
        return pa.int64()
    if "BOOL" in normalized:
        return pa.bool_()
    if any(token in normalized for token in ("REAL", "FLOA", "DOUB", "NUM", "DEC")):
        return pa.float64()
    if "BLOB" in normalized:
        return pa.binary()
    if normalized:
        return pa.string()

    row = conn.execute(
        f"""SELECT typeof({quote_identifier(column)}) AS value_type
        FROM {quote_identifier(table)} {where_sql}
        AND {quote_identifier(column)} IS NOT NULL
        LIMIT 1""",
        params,
    ).fetchone()
    return {
        "integer": pa.int64(),
        "real": pa.float64(),
        "blob": pa.binary(),
        "text": pa.string(),
    }.get(None if row is None else row["value_type"], pa.string())


def get_parquet_schema(
    conn: sqlite3.Connection,
    table: str,
    where_sql: str,
    params: tuple[Any, ...],
    pa: Any,
):
    return pa.schema(
        [
            (column, arrow_type_for(conn, table, column, declared_type, where_sql, params, pa))
            for column, declared_type in get_table_column_types(conn, table).items()
        ]
    )


def export_table_to_parquet(
    source_conn: sqlite3.Connection,
    output_path: Path,
    table: str,
    where_sql: str,
    params: tuple[Any, ...],
    batch_size: int,
) -> int:
    pa, pq = parquet_modules()
    writer = None
    written = 0
    schema = get_parquet_schema(source_conn, table, where_sql, params, pa)

    try:
        for rows in iter_table_batches(source_conn, table, where_sql, params, batch_size):
            arrays = [
                pa.array([row[column] for row in rows], type=schema.field(column).type)
                for column in schema.names
            ]
            arrow_table = pa.Table.from_arrays(arrays, schema=schema)
            if writer is None:
                writer = pq.ParquetWriter(output_path, schema)
            writer.write_table(arrow_table)
            written += len(rows)

        if writer is None:
            arrays = [pa.array([], type=field.type) for field in schema]
            pq.write_table(pa.Table.from_arrays(arrays, schema=schema), output_path)
        return written
    finally:
        if writer is not None:
            writer.close()
