"""Shared SQLite connection and operation-lock policy for NetFlow databases."""

from __future__ import annotations

import fcntl
import sqlite3
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path


BUSY_TIMEOUT_MS = 60_000


class DatabaseOperationLocked(RuntimeError):
    """Raised when an incompatible database-wide operation is active."""


def database_operation_lock_path(path: str | Path) -> Path:
    """Return the stable lock path for a database, whether or not it exists yet."""
    database_path = Path(path).expanduser().resolve()
    return database_path.parent / f'.{database_path.name}.operation.lock'


@contextmanager
def database_operation_lock(path: str | Path, operation: str) -> Iterator[None]:
    """Hold a nonblocking process-lifetime lock for an exclusive DB operation."""
    lock_path = database_operation_lock_path(path)
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    lock = lock_path.open('a+', encoding='utf-8')
    try:
        try:
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            lock.seek(0)
            active_operation = lock.read().strip() or 'unknown operation'
            raise DatabaseOperationLocked(
                f'Cannot start {operation}: {active_operation} is active for {Path(path)}'
            ) from error
        lock.seek(0)
        lock.truncate()
        lock.write(operation)
        lock.flush()
        yield
    finally:
        try:
            fcntl.flock(lock.fileno(), fcntl.LOCK_UN)
        finally:
            lock.close()


def connect_pipeline_writer(
    path: str | Path,
    *,
    busy_timeout_ms: int = BUSY_TIMEOUT_MS,
) -> sqlite3.Connection:
    """Open a pipeline database with reader-friendly journaling and lock waits."""
    conn = sqlite3.connect(path, timeout=busy_timeout_ms / 1000)
    conn.execute(f'PRAGMA busy_timeout = {busy_timeout_ms}')
    conn.execute('PRAGMA journal_mode = WAL')
    return conn


def connect_local_writer(path: str | Path) -> sqlite3.Connection:
    """Open a local single-file database used for temporary extracted output."""
    conn = sqlite3.connect(path, timeout=BUSY_TIMEOUT_MS / 1000)
    conn.execute(f'PRAGMA busy_timeout = {BUSY_TIMEOUT_MS}')
    conn.execute('PRAGMA journal_mode = DELETE')
    return conn


def connect_readonly(path: str | Path) -> sqlite3.Connection:
    """Open an existing database as a strict read-only connection."""
    uri = f'{Path(path).resolve().as_uri()}?mode=ro'
    conn = sqlite3.connect(uri, uri=True, timeout=BUSY_TIMEOUT_MS / 1000)
    conn.execute(f'PRAGMA busy_timeout = {BUSY_TIMEOUT_MS}')
    conn.execute('PRAGMA query_only = ON')
    return conn
