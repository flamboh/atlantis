import json
import sqlite3
import sys
import threading
import time
from contextlib import closing, contextmanager
from pathlib import Path

import pytest

from sqlite_runtime import (
    DatabaseOperationLocked,
    connect_local_writer,
    connect_pipeline_writer,
    connect_readonly,
    database_operation_lock,
)
from extract_window.sqlite import create_source_snapshot


def test_pipeline_writer_commits_while_reader_snapshot_is_open(tmp_path: Path) -> None:
    database_path = tmp_path / 'netflow.sqlite'
    with closing(connect_pipeline_writer(database_path)) as writer:
        writer.execute('CREATE TABLE events (value TEXT NOT NULL)')
        writer.execute("INSERT INTO events VALUES ('before')")
        writer.commit()

        with closing(connect_readonly(database_path)) as reader:
            reader.execute('BEGIN')
            assert reader.execute('SELECT value FROM events').fetchall() == [('before',)]

            writer.execute("INSERT INTO events VALUES ('after')")
            writer.commit()

            assert reader.execute('SELECT value FROM events').fetchall() == [('before',)]

    with sqlite3.connect(database_path) as verification:
        assert verification.execute('SELECT value FROM events ORDER BY rowid').fetchall() == [
            ('before',),
            ('after',),
        ]


def test_database_operation_lock_is_nonblocking_and_releases(tmp_path: Path) -> None:
    database_path = tmp_path / 'netflow.sqlite'

    with database_operation_lock(database_path, 'pipeline build'):
        with pytest.raises(DatabaseOperationLocked, match='pipeline build'):
            with database_operation_lock(database_path, 'source snapshot'):
                pass

    with database_operation_lock(database_path, 'source snapshot'):
        pass


def test_local_writer_keeps_extracted_database_single_file(tmp_path: Path) -> None:
    database_path = tmp_path / 'extract.sqlite'

    with closing(connect_local_writer(database_path)) as writer:
        assert writer.execute('PRAGMA journal_mode').fetchone() == ('delete',)
        writer.execute('CREATE TABLE events (value TEXT NOT NULL)')
        writer.commit()

    assert not database_path.with_name(f'{database_path.name}-wal').exists()
    assert not database_path.with_name(f'{database_path.name}-shm').exists()


def test_pipeline_writer_waits_for_a_brief_competing_writer(tmp_path: Path) -> None:
    database_path = tmp_path / 'netflow.sqlite'
    with closing(connect_pipeline_writer(database_path)) as setup:
        setup.execute('CREATE TABLE events (value TEXT NOT NULL)')
        setup.commit()

    started = threading.Event()

    def hold_write_lock() -> None:
        with closing(connect_pipeline_writer(database_path)) as holder:
            holder.execute('BEGIN IMMEDIATE')
            holder.execute("INSERT INTO events VALUES ('first')")
            started.set()
            time.sleep(0.15)
            holder.commit()

    thread = threading.Thread(target=hold_write_lock)
    thread.start()
    assert started.wait(timeout=2)

    start = time.monotonic()
    with closing(connect_pipeline_writer(database_path)) as writer:
        writer.execute("INSERT INTO events VALUES ('second')")
        writer.commit()
    elapsed = time.monotonic() - start
    thread.join(timeout=2)

    assert not thread.is_alive()
    assert elapsed >= 0.1


def test_readonly_connection_rejects_writes_and_has_busy_timeout(tmp_path: Path) -> None:
    database_path = tmp_path / 'netflow.sqlite'
    with closing(connect_pipeline_writer(database_path)) as writer:
        writer.execute('CREATE TABLE events (value TEXT NOT NULL)')
        writer.commit()

    with closing(connect_readonly(database_path)) as reader:
        assert reader.execute('PRAGMA query_only').fetchone() == (1,)
        assert reader.execute('PRAGMA busy_timeout').fetchone() == (60_000,)
        with pytest.raises(sqlite3.OperationalError, match='readonly'):
            reader.execute("INSERT INTO events VALUES ('forbidden')")


def test_pipeline_main_holds_database_operation_lock_before_opening_database(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import pipeline

    database_path = tmp_path / 'netflow.sqlite'
    config_path = tmp_path / 'pipeline.json'
    config_path.write_text(
        json.dumps({'database_path': str(database_path), 'inputs': []}),
        encoding='utf-8',
    )
    monkeypatch.setattr(sys, 'argv', ['pipeline.py', '--config', str(config_path)])
    monkeypatch.setattr(
        pipeline,
        'process_pipeline_config',
        lambda _connection, _config: pytest.fail('pipeline opened a locked database'),
    )

    with database_operation_lock(database_path, 'source snapshot'):
        with pytest.raises(DatabaseOperationLocked, match='source snapshot'):
            pipeline.main()


def test_pipeline_main_closes_connection_before_releasing_operation_lock(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import pipeline

    database_path = tmp_path / 'netflow.sqlite'
    config_path = tmp_path / 'pipeline.json'
    config_path.write_text(
        json.dumps({'database_path': str(database_path), 'inputs': []}),
        encoding='utf-8',
    )
    monkeypatch.setattr(sys, 'argv', ['pipeline.py', '--config', str(config_path)])
    connection_closed = False

    class FakeConnection:
        def close(self) -> None:
            nonlocal connection_closed
            connection_closed = True

    @contextmanager
    def tracked_operation_lock(_path: Path, _operation: str):
        yield
        assert connection_closed

    monkeypatch.setattr(pipeline, 'database_operation_lock', tracked_operation_lock)
    monkeypatch.setattr(pipeline, 'connect_pipeline_writer', lambda _path: FakeConnection())
    monkeypatch.setattr(pipeline, 'process_pipeline_config', lambda _conn, _config: None)

    pipeline.main()

    assert connection_closed


def test_source_snapshot_fails_clearly_while_pipeline_build_is_active(tmp_path: Path) -> None:
    database_path = tmp_path / 'netflow.sqlite'
    snapshot_path = tmp_path / 'snapshot.sqlite'
    with closing(connect_pipeline_writer(database_path)) as writer:
        writer.execute('CREATE TABLE events (value TEXT NOT NULL)')
        writer.commit()

    with database_operation_lock(database_path, 'pipeline build'):
        with pytest.raises(DatabaseOperationLocked, match='pipeline build'):
            create_source_snapshot(database_path, snapshot_path)

    assert not snapshot_path.exists()
