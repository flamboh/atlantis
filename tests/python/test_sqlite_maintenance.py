import importlib
import sqlite3
from contextlib import closing
from pathlib import Path

import pytest

from sqlite_runtime import (
    DatabaseOperationLocked,
    connect_pipeline_writer,
    database_operation_lock,
)


def load_module():
    return importlib.reload(importlib.import_module('sqlite_maintenance'))


def test_backup_database_captures_committed_rows_from_active_wal(tmp_path: Path) -> None:
    maintenance = load_module()
    source_path = tmp_path / 'source.sqlite'
    target_path = tmp_path / 'backup.sqlite'

    with closing(connect_pipeline_writer(source_path)) as writer:
        writer.execute('CREATE TABLE events (value TEXT NOT NULL)')
        writer.execute("INSERT INTO events VALUES ('committed-in-wal')")
        writer.commit()
        assert source_path.with_name(f'{source_path.name}-wal').exists()

        maintenance.backup_database(source_path, target_path)

    with sqlite3.connect(target_path) as verification:
        assert verification.execute('PRAGMA quick_check').fetchone() == ('ok',)
        assert verification.execute('SELECT value FROM events').fetchall() == [
            ('committed-in-wal',)
        ]


def test_backup_database_refuses_an_active_lifetime_operation(tmp_path: Path) -> None:
    maintenance = load_module()
    source_path = tmp_path / 'source.sqlite'
    target_path = tmp_path / 'backup.sqlite'
    with closing(connect_pipeline_writer(source_path)) as writer:
        writer.execute('CREATE TABLE events (value TEXT NOT NULL)')
        writer.commit()

    with database_operation_lock(source_path, 'pipeline build'):
        with pytest.raises(DatabaseOperationLocked, match='pipeline build'):
            maintenance.backup_database(source_path, target_path)

    assert not target_path.exists()


def test_backup_database_atomically_replaces_existing_target(tmp_path: Path) -> None:
    maintenance = load_module()
    source_path = tmp_path / 'source.sqlite'
    target_path = tmp_path / 'target.sqlite'
    for path, value in ((source_path, 'new'), (target_path, 'old')):
        with sqlite3.connect(path) as conn:
            conn.execute('CREATE TABLE events (value TEXT NOT NULL)')
            conn.execute('INSERT INTO events VALUES (?)', (value,))
            conn.commit()
    stale_sidecars = [
        target_path.with_name(f'{target_path.name}{suffix}')
        for suffix in ('-journal', '-wal', '-shm')
    ]
    for sidecar in stale_sidecars:
        sidecar.write_bytes(b'stale')

    maintenance.backup_database(source_path, target_path)

    with sqlite3.connect(target_path) as verification:
        assert verification.execute('SELECT value FROM events').fetchall() == [('new',)]
    assert not any(sidecar.exists() for sidecar in stale_sidecars)
    assert list(tmp_path.glob('.target.sqlite.*.tmp')) == []


def test_promote_database_preserves_existing_target_and_active_wal_candidate(
    tmp_path: Path,
) -> None:
    maintenance = load_module()
    candidate_path = tmp_path / 'candidate.sqlite'
    target_path = tmp_path / 'target.sqlite'
    backup_path = tmp_path / 'target.backup.sqlite'

    with closing(connect_pipeline_writer(target_path)) as current_writer:
        current_writer.execute('CREATE TABLE events (value TEXT NOT NULL)')
        current_writer.execute("INSERT INTO events VALUES ('old')")
        current_writer.commit()

    with closing(connect_pipeline_writer(candidate_path)) as candidate_writer:
        candidate_writer.execute('CREATE TABLE events (value TEXT NOT NULL)')
        candidate_writer.execute("INSERT INTO events VALUES ('new-in-wal')")
        candidate_writer.commit()

        maintenance.promote_database(
            candidate_path,
            target_path,
            backup_existing_path=backup_path,
        )

    with sqlite3.connect(target_path) as promoted:
        assert promoted.execute('SELECT value FROM events').fetchall() == [('new-in-wal',)]
    with sqlite3.connect(backup_path) as backup:
        assert backup.execute('SELECT value FROM events').fetchall() == [('old',)]
    assert candidate_path.exists()
