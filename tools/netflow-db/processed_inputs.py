"""
Input tracking table for pipeline.

Tracks logical bucket-level inputs across nfcapd and CSV sources.
"""

from __future__ import annotations

import sqlite3
from dataclasses import dataclass


VALID_INPUT_STATUSES = {'pending', 'processed', 'failed'}


@dataclass(frozen=True, slots=True)
class InputBucketRef:
    source_id: str
    bucket_start: int


def init_processed_inputs_table(conn: sqlite3.Connection) -> None:
    """Create the processed_inputs table if it does not exist."""
    existing_columns = {
        row[1]
        for row in conn.execute('PRAGMA table_info(processed_inputs)').fetchall()
    }
    migrates_legacy_csv_tracking = bool(existing_columns) and 'scan_locator' not in existing_columns
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS processed_inputs (
            input_kind TEXT NOT NULL CHECK (input_kind IN ('nfcapd', 'csv')),
            input_locator TEXT NOT NULL,
            scan_locator TEXT NOT NULL,
            source_id TEXT NOT NULL,
            bucket_start INTEGER NOT NULL,
            bucket_end INTEGER NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'processed', 'failed')),
            error_message TEXT,
            discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            processed_at DATETIME,
            PRIMARY KEY (input_kind, input_locator, source_id, bucket_start)
        ) WITHOUT ROWID
        """
    )
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS processed_input_scans (
            input_kind TEXT NOT NULL CHECK (input_kind IN ('csv')),
            input_locator TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('processed')),
            rejected_rows INTEGER NOT NULL DEFAULT 0,
            skipped_bad_column_count INTEGER NOT NULL DEFAULT 0,
            processed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (input_kind, input_locator)
        ) WITHOUT ROWID
        """
    )
    ensure_column(conn, 'processed_inputs', 'status', "TEXT NOT NULL DEFAULT 'pending'")
    ensure_column(conn, 'processed_inputs', 'error_message', 'TEXT')
    ensure_column(conn, 'processed_inputs', 'processed_at', 'DATETIME')
    ensure_column(conn, 'processed_inputs', 'scan_locator', 'TEXT')
    ensure_column(
        conn,
        'processed_input_scans',
        'skipped_bad_column_count',
        'INTEGER NOT NULL DEFAULT 0',
    )
    conn.execute(
        """
        UPDATE processed_inputs
        SET scan_locator = input_locator
        WHERE scan_locator IS NULL
        """
    )
    if migrates_legacy_csv_tracking:
        conn.execute(
            """
            INSERT INTO processed_input_scans (
                input_kind, input_locator, status, rejected_rows,
                skipped_bad_column_count, processed_at
            )
            SELECT 'csv', scan_locator, 'processed', 0, 0, MAX(processed_at)
            FROM processed_inputs
            WHERE input_kind = 'csv'
            GROUP BY scan_locator
            HAVING COUNT(*) > 0
               AND SUM(CASE WHEN status != 'processed' THEN 1 ELSE 0 END) = 0
            ON CONFLICT(input_kind, input_locator) DO NOTHING
            """
        )
    conn.execute(
        """
        CREATE INDEX IF NOT EXISTS idx_processed_inputs_source_bucket
        ON processed_inputs(source_id, bucket_start)
        """
    )
    conn.execute(
        """
        CREATE INDEX IF NOT EXISTS idx_processed_inputs_scan_status
        ON processed_inputs(input_kind, scan_locator, status)
        """
    )


def ensure_column(conn: sqlite3.Connection, table_name: str, column_name: str, column_type: str) -> None:
    """Add a column to an existing table when needed."""
    columns = {
        row[1]
        for row in conn.execute(f'PRAGMA table_info({table_name})').fetchall()
    }
    if column_name not in columns:
        conn.execute(f'ALTER TABLE {table_name} ADD COLUMN {column_name} {column_type}')


def upsert_input_bucket(
    conn: sqlite3.Connection,
    *,
    input_kind: str,
    input_locator: str,
    scan_locator: str | None = None,
    source_id: str,
    bucket_start: int,
    bucket_end: int,
) -> None:
    """Insert or replace an input bucket record without committing."""
    init_processed_inputs_table(conn)
    conn.execute(
        """
        INSERT INTO processed_inputs (
            input_kind, input_locator, scan_locator, source_id, bucket_start, bucket_end, status
        ) VALUES (?, ?, ?, ?, ?, ?, 'pending')
        ON CONFLICT(input_kind, input_locator, source_id, bucket_start)
        DO UPDATE SET
            scan_locator = excluded.scan_locator,
            bucket_end = excluded.bucket_end,
            status = 'pending',
            error_message = NULL,
            processed_at = NULL
        """,
        (
            input_kind,
            input_locator,
            scan_locator if scan_locator is not None else input_locator,
            source_id,
            bucket_start,
            bucket_end,
        ),
    )


def mark_input_bucket_status(
    conn: sqlite3.Connection,
    *,
    input_kind: str,
    input_locator: str,
    source_id: str,
    bucket_start: int,
    status: str,
    error_message: str | None = None,
) -> None:
    """Mark one input bucket status without committing."""
    if status not in VALID_INPUT_STATUSES:
        raise ValueError(f'Unsupported input status: {status}')
    conn.execute(
        """
        UPDATE processed_inputs
        SET status = ?,
            error_message = ?,
            processed_at = CURRENT_TIMESTAMP
        WHERE input_kind = ? AND input_locator = ? AND source_id = ? AND bucket_start = ?
        """,
        (status, error_message, input_kind, input_locator, source_id, bucket_start),
    )


def _mark_input_scan_processed(
    conn: sqlite3.Connection,
    *,
    input_kind: str,
    input_locator: str,
    rejected_rows: int,
    skipped_bad_column_count: int,
) -> None:
    """Record successful whole-input completion without committing."""
    if input_kind != 'csv':
        raise ValueError(f'Unsupported scanned input kind: {input_kind}')
    if rejected_rows < 0:
        raise ValueError('rejected_rows must be non-negative')
    if skipped_bad_column_count < 0:
        raise ValueError('skipped_bad_column_count must be non-negative')
    init_processed_inputs_table(conn)
    conn.execute(
        """
        INSERT INTO processed_input_scans (
            input_kind, input_locator, status, rejected_rows,
            skipped_bad_column_count, processed_at
        ) VALUES (?, ?, 'processed', ?, ?, CURRENT_TIMESTAMP)
        ON CONFLICT(input_kind, input_locator) DO UPDATE SET
            status = 'processed',
            rejected_rows = excluded.rejected_rows,
            skipped_bad_column_count = excluded.skipped_bad_column_count,
            processed_at = CURRENT_TIMESTAMP
        """,
        (input_kind, input_locator, rejected_rows, skipped_bad_column_count),
    )


def complete_input_scan(
    conn: sqlite3.Connection,
    *,
    input_kind: str,
    scan_locator: str,
    rejected_rows: int,
    skipped_bad_column_count: int = 0,
) -> None:
    """Atomically publish a successfully scanned input after its buckets are processed."""
    init_processed_inputs_table(conn)
    unfinished = conn.execute(
        """
        SELECT COUNT(*)
        FROM processed_inputs
        WHERE input_kind = ?
          AND scan_locator = ?
          AND status != 'processed'
        """,
        (input_kind, scan_locator),
    ).fetchone()[0]
    if unfinished:
        raise ValueError(
            f'Cannot complete input scan {scan_locator!r}: {unfinished} bucket(s) are unfinished.'
        )
    _mark_input_scan_processed(
        conn,
        input_kind=input_kind,
        input_locator=scan_locator,
        rejected_rows=rejected_rows,
        skipped_bad_column_count=skipped_bad_column_count,
    )


def input_scan_fully_processed(
    conn: sqlite3.Connection,
    *,
    input_kind: str,
    scan_locator: str,
) -> bool:
    """Return whether a successful scan and all of its bucket publications are complete."""
    init_processed_inputs_table(conn)
    return conn.execute(
        """
        SELECT 1
        FROM processed_input_scans AS scans
        WHERE scans.input_kind = ?
          AND scans.input_locator = ?
          AND scans.status = 'processed'
          AND NOT EXISTS (
              SELECT 1
              FROM processed_inputs AS buckets
              WHERE buckets.input_kind = scans.input_kind
                AND buckets.scan_locator = scans.input_locator
                AND buckets.status != 'processed'
          )
        """,
        (input_kind, scan_locator),
    ).fetchone() is not None


def clear_incomplete_input_scan(
    conn: sqlite3.Connection,
    *,
    input_kind: str,
    scan_locator: str,
) -> list[InputBucketRef]:
    """Remove bucket tracking from a non-terminal scan attempt without committing."""
    init_processed_inputs_table(conn)
    terminal = conn.execute(
        """
        SELECT 1
        FROM processed_input_scans
        WHERE input_kind = ?
          AND input_locator = ?
          AND status = 'processed'
        """,
        (input_kind, scan_locator),
    ).fetchone()
    if terminal is not None:
        raise ValueError(f'Cannot clear successfully completed input scan {scan_locator!r}.')

    buckets = [
        InputBucketRef(str(row[0]), int(row[1]))
        for row in conn.execute(
            """
            SELECT DISTINCT source_id, bucket_start
            FROM processed_inputs
            WHERE input_kind = ? AND scan_locator = ?
            ORDER BY source_id, bucket_start
            """,
            (input_kind, scan_locator),
        ).fetchall()
    ]
    conn.execute(
        """
        DELETE FROM processed_inputs
        WHERE input_kind = ? AND scan_locator = ?
        """,
        (input_kind, scan_locator),
    )
    return buckets
