#!/usr/bin/env python3
"""Lock-aware, WAL-safe SQLite backup and atomic publication."""

from __future__ import annotations

import argparse
import os
import tempfile
from collections.abc import Iterator
from contextlib import ExitStack, closing, contextmanager
from pathlib import Path

from sqlite_runtime import (
    connect_local_writer,
    connect_readonly,
    database_operation_lock,
)


def backup_database(source_path: Path, target_path: Path) -> None:
    """Publish a consistent SQLite backup without moving a live main file."""
    source_path, target_path = resolved_backup_paths(source_path, target_path)
    with database_operation_locks(
        (source_path, target_path),
        f'database backup {source_path} -> {target_path}',
    ):
        publish_backup(source_path, target_path)


def resolved_backup_paths(source_path: Path, target_path: Path) -> tuple[Path, Path]:
    source_path = source_path.expanduser().resolve()
    target_path = target_path.expanduser().resolve()
    if source_path == target_path:
        raise ValueError('Source and target database paths must differ.')
    if not source_path.is_file():
        raise FileNotFoundError(f'Database not found: {source_path}')
    return source_path, target_path


@contextmanager
def database_operation_locks(paths: tuple[Path, ...], operation: str) -> Iterator[None]:
    """Acquire database operation locks in stable order to avoid deadlocks."""
    with ExitStack() as stack:
        for path in sorted(set(paths)):
            stack.enter_context(database_operation_lock(path, operation))
        yield


def publish_backup(source_path: Path, target_path: Path) -> None:
    """Create, validate, and atomically publish one backup with locks already held."""

    target_path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f'.{target_path.name}.',
        suffix='.tmp',
        dir=target_path.parent,
    )
    os.close(descriptor)
    temporary_path = Path(temporary_name)
    try:
        with closing(connect_readonly(source_path)) as source_conn:
            with closing(connect_local_writer(temporary_path)) as target_conn:
                source_conn.backup(target_conn)
                result = target_conn.execute('PRAGMA quick_check').fetchone()
                if result != ('ok',):
                    raise RuntimeError(f'Backup quick_check failed: {result!r}')
        atomic_replace_sqlite(temporary_path, target_path)
    finally:
        temporary_path.unlink(missing_ok=True)
        for suffix in ('-journal', '-wal', '-shm'):
            temporary_path.with_name(f'{temporary_path.name}{suffix}').unlink(missing_ok=True)


def atomic_replace_sqlite(source_path: Path, target_path: Path) -> None:
    """Replace a main database without exposing stale target journal sidecars."""
    displaced_sidecars: list[tuple[Path, Path]] = []
    try:
        for suffix in ('-journal', '-wal', '-shm'):
            sidecar_path = target_path.with_name(f'{target_path.name}{suffix}')
            if not sidecar_path.exists():
                continue
            displaced_path = source_path.with_name(
                f'{source_path.name}.previous{suffix}'
            )
            os.replace(sidecar_path, displaced_path)
            displaced_sidecars.append((sidecar_path, displaced_path))
        os.replace(source_path, target_path)
    except BaseException:
        for sidecar_path, displaced_path in reversed(displaced_sidecars):
            os.replace(displaced_path, sidecar_path)
        raise
    for _, displaced_path in displaced_sidecars:
        displaced_path.unlink(missing_ok=True)


def promote_database(
    candidate_path: Path,
    target_path: Path,
    *,
    backup_existing_path: Path | None = None,
) -> None:
    """Back up the current target, then atomically publish a candidate snapshot."""
    candidate_path, target_path = resolved_backup_paths(candidate_path, target_path)
    resolved_backup_existing = (
        backup_existing_path.expanduser().resolve()
        if backup_existing_path is not None
        else None
    )
    lock_paths = (candidate_path, target_path)
    if resolved_backup_existing is not None:
        lock_paths += (resolved_backup_existing,)
    with database_operation_locks(lock_paths, f'database promotion to {target_path}'):
        if target_path.exists() and resolved_backup_existing is not None:
            publish_backup(target_path, resolved_backup_existing)
        publish_backup(candidate_path, target_path)


def main() -> None:
    parser = argparse.ArgumentParser(
        description='Create and atomically publish a lock-aware SQLite backup.'
    )
    parser.add_argument('source_path', type=Path)
    parser.add_argument('target_path', type=Path)
    parser.add_argument('--backup-existing', type=Path)
    args = parser.parse_args()
    promote_database(
        args.source_path,
        args.target_path,
        backup_existing_path=args.backup_existing,
    )
    print(f'published SQLite database: {args.source_path} -> {args.target_path}')


if __name__ == '__main__':
    main()
