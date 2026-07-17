"""
Input tracking table for pipeline.

Tracks logical bucket-level inputs across nfcapd and CSV sources.
"""

from __future__ import annotations

import sqlite3
from dataclasses import dataclass

from input_revision import FileSnapshot, InputRevision


VALID_INPUT_STATUSES = {'pending', 'processed', 'failed'}


@dataclass(frozen=True, slots=True)
class InputBucketRef:
    source_id: str
    bucket_start: int


class InputRevisionConflict(ValueError):
    """Raised when one locator is reused with different content or decoding."""


def init_processed_inputs_table(conn: sqlite3.Connection) -> None:
    """Create the processed_inputs table if it does not exist."""
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
            content_fingerprint TEXT NOT NULL,
            decoder_fingerprint TEXT NOT NULL,
            revision_fingerprint TEXT NOT NULL,
            file_device INTEGER,
            file_inode INTEGER,
            file_size INTEGER,
            file_mtime_ns INTEGER,
            file_ctime_ns INTEGER,
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
            content_fingerprint TEXT NOT NULL,
            decoder_fingerprint TEXT NOT NULL,
            revision_fingerprint TEXT NOT NULL,
            file_device INTEGER,
            file_inode INTEGER,
            file_size INTEGER,
            file_mtime_ns INTEGER,
            file_ctime_ns INTEGER,
            PRIMARY KEY (input_kind, input_locator)
        ) WITHOUT ROWID
        """
    )
    ensure_column(conn, 'processed_inputs', 'status', "TEXT NOT NULL DEFAULT 'pending'")
    ensure_column(conn, 'processed_inputs', 'error_message', 'TEXT')
    ensure_column(conn, 'processed_inputs', 'processed_at', 'DATETIME')
    ensure_column(conn, 'processed_inputs', 'scan_locator', 'TEXT')
    ensure_column(conn, 'processed_inputs', 'content_fingerprint', 'TEXT')
    ensure_column(conn, 'processed_inputs', 'decoder_fingerprint', 'TEXT')
    ensure_column(conn, 'processed_inputs', 'revision_fingerprint', 'TEXT')
    ensure_column(conn, 'processed_input_scans', 'content_fingerprint', 'TEXT')
    ensure_column(conn, 'processed_input_scans', 'decoder_fingerprint', 'TEXT')
    ensure_column(conn, 'processed_input_scans', 'revision_fingerprint', 'TEXT')
    for table_name in ('processed_inputs', 'processed_input_scans'):
        ensure_column(conn, table_name, 'file_device', 'INTEGER')
        ensure_column(conn, table_name, 'file_inode', 'INTEGER')
        ensure_column(conn, table_name, 'file_size', 'INTEGER')
        ensure_column(conn, table_name, 'file_mtime_ns', 'INTEGER')
        ensure_column(conn, table_name, 'file_ctime_ns', 'INTEGER')
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
    input_revision: InputRevision,
    file_snapshot: FileSnapshot | None = None,
    replace_revision: bool = False,
) -> None:
    """Insert or replace an input bucket record without committing."""
    init_processed_inputs_table(conn)
    _validate_revision_identity(input_revision, input_kind, input_locator)
    existing = conn.execute(
        """
        SELECT content_fingerprint, decoder_fingerprint, revision_fingerprint
        FROM processed_inputs
        WHERE input_kind = ? AND input_locator = ? AND source_id = ? AND bucket_start = ?
        """,
        (input_kind, input_locator, source_id, bucket_start),
    ).fetchone()
    if existing is not None and existing[2] != input_revision.fingerprint and not replace_revision:
        _raise_revision_conflict(input_locator, existing, input_revision)
    conn.execute(
        """
        INSERT INTO processed_inputs (
            input_kind, input_locator, scan_locator, source_id, bucket_start, bucket_end, status,
            content_fingerprint, decoder_fingerprint, revision_fingerprint,
            file_device, file_inode, file_size, file_mtime_ns, file_ctime_ns
        ) VALUES (?, ?, ?, ?, ?, ?, 'pending', ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(input_kind, input_locator, source_id, bucket_start)
        DO UPDATE SET
            scan_locator = excluded.scan_locator,
            bucket_end = excluded.bucket_end,
            status = 'pending',
            error_message = NULL,
            processed_at = NULL,
            content_fingerprint = excluded.content_fingerprint,
            decoder_fingerprint = excluded.decoder_fingerprint,
            revision_fingerprint = excluded.revision_fingerprint,
            file_device = excluded.file_device,
            file_inode = excluded.file_inode,
            file_size = excluded.file_size,
            file_mtime_ns = excluded.file_mtime_ns,
            file_ctime_ns = excluded.file_ctime_ns
        """,
        (
            input_kind,
            input_locator,
            scan_locator if scan_locator is not None else input_locator,
            source_id,
            bucket_start,
            bucket_end,
            input_revision.content_fingerprint,
            input_revision.decoder_fingerprint,
            input_revision.fingerprint,
            *(snapshot_values(file_snapshot)),
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
    input_revision: InputRevision,
    error_message: str | None = None,
) -> None:
    """Mark one input bucket status without committing."""
    if status not in VALID_INPUT_STATUSES:
        raise ValueError(f'Unsupported input status: {status}')
    _validate_revision_identity(input_revision, input_kind, input_locator)
    cursor = conn.execute(
        """
        UPDATE processed_inputs
        SET status = ?,
            error_message = ?,
            processed_at = CURRENT_TIMESTAMP
        WHERE input_kind = ? AND input_locator = ? AND source_id = ? AND bucket_start = ?
          AND revision_fingerprint = ?
        """,
        (
            status,
            error_message,
            input_kind,
            input_locator,
            source_id,
            bucket_start,
            input_revision.fingerprint,
        ),
    )
    if cursor.rowcount != 1:
        raise InputRevisionConflict(
            f'Input revision changed before status update for {input_locator!r}.'
        )


def _mark_input_scan_processed(
    conn: sqlite3.Connection,
    *,
    input_kind: str,
    input_locator: str,
    rejected_rows: int,
    skipped_bad_column_count: int,
    input_revision: InputRevision,
    file_snapshot: FileSnapshot | None,
) -> None:
    """Record successful whole-input completion without committing."""
    if input_kind != 'csv':
        raise ValueError(f'Unsupported scanned input kind: {input_kind}')
    if rejected_rows < 0:
        raise ValueError('rejected_rows must be non-negative')
    if skipped_bad_column_count < 0:
        raise ValueError('skipped_bad_column_count must be non-negative')
    init_processed_inputs_table(conn)
    _validate_revision_identity(input_revision, input_kind, input_locator)
    existing = conn.execute(
        """
        SELECT content_fingerprint, decoder_fingerprint, revision_fingerprint
        FROM processed_input_scans WHERE input_kind = ? AND input_locator = ?
        """,
        (input_kind, input_locator),
    ).fetchone()
    if existing is not None and existing[2] != input_revision.fingerprint:
        _raise_revision_conflict(input_locator, existing, input_revision)
    conn.execute(
        """
        INSERT INTO processed_input_scans (
            input_kind, input_locator, status, rejected_rows,
            skipped_bad_column_count, processed_at,
            content_fingerprint, decoder_fingerprint, revision_fingerprint,
            file_device, file_inode, file_size, file_mtime_ns, file_ctime_ns
        ) VALUES (?, ?, 'processed', ?, ?, CURRENT_TIMESTAMP, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(input_kind, input_locator) DO UPDATE SET
            status = 'processed',
            rejected_rows = excluded.rejected_rows,
            skipped_bad_column_count = excluded.skipped_bad_column_count,
            processed_at = CURRENT_TIMESTAMP,
            content_fingerprint = excluded.content_fingerprint,
            decoder_fingerprint = excluded.decoder_fingerprint,
            revision_fingerprint = excluded.revision_fingerprint,
            file_device = excluded.file_device,
            file_inode = excluded.file_inode,
            file_size = excluded.file_size,
            file_mtime_ns = excluded.file_mtime_ns,
            file_ctime_ns = excluded.file_ctime_ns
        """,
        (
            input_kind,
            input_locator,
            rejected_rows,
            skipped_bad_column_count,
            input_revision.content_fingerprint,
            input_revision.decoder_fingerprint,
            input_revision.fingerprint,
            *(snapshot_values(file_snapshot)),
        ),
    )


def complete_input_scan(
    conn: sqlite3.Connection,
    *,
    input_kind: str,
    scan_locator: str,
    rejected_rows: int,
    skipped_bad_column_count: int = 0,
    input_revision: InputRevision,
    file_snapshot: FileSnapshot | None = None,
) -> None:
    """Atomically publish a successfully scanned input after its buckets are processed."""
    init_processed_inputs_table(conn)
    unfinished = conn.execute(
        """
        SELECT COUNT(*)
        FROM processed_inputs
        WHERE input_kind = ?
          AND scan_locator = ?
          AND (
              status != 'processed'
              OR content_fingerprint != ?
              OR decoder_fingerprint != ?
          )
        """,
        (
            input_kind,
            scan_locator,
            input_revision.content_fingerprint,
            input_revision.decoder_fingerprint,
        ),
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
        input_revision=input_revision,
        file_snapshot=file_snapshot,
    )


def snapshot_values(snapshot: FileSnapshot | None) -> tuple[int | None, ...]:
    """Return SQLite column values for an optional file snapshot."""
    if snapshot is None:
        return (None, None, None, None, None)
    return (
        snapshot.device,
        snapshot.inode,
        snapshot.size,
        snapshot.mtime_ns,
        snapshot.ctime_ns,
    )


def cached_content_fingerprint(
    conn: sqlite3.Connection,
    *,
    input_kind: str,
    input_locator: str,
    file_snapshot: FileSnapshot,
) -> str | None:
    """Reuse an exact digest only for a completed input with identical file identity."""
    init_processed_inputs_table(conn)
    table_name = 'processed_input_scans' if input_kind == 'csv' else 'processed_inputs'
    row = conn.execute(
        f"""
        SELECT content_fingerprint
        FROM {table_name}
        WHERE input_kind = ? AND input_locator = ? AND status = 'processed'
          AND file_device = ? AND file_inode = ? AND file_size = ?
          AND file_mtime_ns = ? AND file_ctime_ns = ?
        LIMIT 1
        """,
        (input_kind, input_locator, *snapshot_values(file_snapshot)),
    ).fetchone()
    return None if row is None else str(row[0])


def input_scan_fully_processed(
    conn: sqlite3.Connection,
    *,
    input_kind: str,
    scan_locator: str,
    input_revision: InputRevision,
) -> bool:
    """Return whether a successful scan and all of its bucket publications are complete."""
    init_processed_inputs_table(conn)
    _validate_revision_identity(input_revision, input_kind, scan_locator)
    row = conn.execute(
        """
        SELECT content_fingerprint, decoder_fingerprint, revision_fingerprint
        FROM processed_input_scans
        WHERE input_kind = ? AND input_locator = ?
        """,
        (input_kind, scan_locator),
    ).fetchone()
    if row is None:
        return False
    if row[2] != input_revision.fingerprint:
        _raise_revision_conflict(scan_locator, row, input_revision)
    return conn.execute(
        """
        SELECT 1
        FROM processed_input_scans AS scans
        WHERE scans.input_kind = ?
          AND scans.input_locator = ?
          AND scans.revision_fingerprint = ?
          AND scans.status = 'processed'
          AND NOT EXISTS (
              SELECT 1
              FROM processed_inputs AS buckets
              WHERE buckets.input_kind = scans.input_kind
                AND buckets.scan_locator = scans.input_locator
                AND (
                    buckets.content_fingerprint != scans.content_fingerprint
                    OR buckets.decoder_fingerprint != scans.decoder_fingerprint
                    OR buckets.status != 'processed'
                )
          )
        """,
        (input_kind, scan_locator, input_revision.fingerprint),
    ).fetchone() is not None


def _validate_revision_identity(
    revision: InputRevision,
    input_kind: str,
    locator: str,
) -> None:
    if revision.input_kind != input_kind or revision.locator != locator:
        raise ValueError(
            'Input revision identity does not match persistence owner: '
            f'{revision.input_kind}:{revision.locator} != {input_kind}:{locator}'
        )


def _raise_revision_conflict(
    locator: str,
    stored: tuple[str | None, str | None, str | None],
    requested: InputRevision,
) -> None:
    mismatches = []
    if stored[0] != requested.content_fingerprint:
        mismatches.append('content')
    if stored[1] != requested.decoder_fingerprint:
        mismatches.append('decoder')
    if not mismatches:
        mismatches.append('combined revision')
    raise InputRevisionConflict(
        f"Input revision mismatch for {locator!r}: {', '.join(mismatches)} changed."
    )


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
