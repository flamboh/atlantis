"""
Input tracking table for pipeline.

Tracks logical bucket-level inputs across nfcapd and CSV sources.
"""

from __future__ import annotations

import sqlite3


VALID_INPUT_STATUSES = {'pending', 'processed', 'failed'}


def init_processed_inputs_table(conn: sqlite3.Connection) -> None:
    """Create the processed_inputs table if it does not exist."""
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS processed_inputs (
            input_kind TEXT NOT NULL CHECK (input_kind IN ('nfcapd', 'csv')),
            input_locator TEXT NOT NULL,
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
    ensure_column(conn, 'processed_inputs', 'status', "TEXT NOT NULL DEFAULT 'pending'")
    ensure_column(conn, 'processed_inputs', 'error_message', 'TEXT')
    ensure_column(conn, 'processed_inputs', 'processed_at', 'DATETIME')
    conn.execute(
        """
        CREATE INDEX IF NOT EXISTS idx_processed_inputs_source_bucket
        ON processed_inputs(source_id, bucket_start)
        """
    )
    conn.commit()


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
    source_id: str,
    bucket_start: int,
    bucket_end: int,
) -> None:
    """Insert or replace an input bucket record without committing."""
    init_processed_inputs_table(conn)
    conn.execute(
        """
        INSERT INTO processed_inputs (
            input_kind, input_locator, source_id, bucket_start, bucket_end, status
        ) VALUES (?, ?, ?, ?, ?, 'pending')
        ON CONFLICT(input_kind, input_locator, source_id, bucket_start)
        DO UPDATE SET
            bucket_end = excluded.bucket_end,
            status = 'pending',
            error_message = NULL,
            processed_at = NULL
        """,
        (input_kind, input_locator, source_id, bucket_start, bucket_end),
    )


def get_pending_inputs(conn: sqlite3.Connection) -> list[dict]:
    """Return bucket inputs that have not completed processing."""
    rows = conn.execute(
        """
        SELECT input_kind, input_locator, source_id, bucket_start, bucket_end
        FROM processed_inputs
        WHERE status != 'processed'
        ORDER BY bucket_start, source_id, input_locator
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
    ]


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
